//! An OpenAI-compatible endpoint over HTTPS, on a maintained TLS client.
//!
//! TLS is `native-tls`: the platform's own implementation and trust store (schannel on Windows,
//! Security Framework on macOS, OpenSSL on Linux). A custom root certificate can be added for an
//! enterprise proxy or a test server; nothing disables verification.
//!
//! Credentials: the key is read from an environment variable the user names, never from argv,
//! and it never appears in `describe`, in an error message, or in any record. An error body that
//! echoes the key back is redacted before it is shown. Retries are bounded (three attempts) and
//! only for failures where trying again is honest: a connection that never answered, a timeout,
//! or a 429/5xx status. A 4xx refusal is not retried.

use super::wire::{self, Endpoint};
use super::{ModelProvider, ModelRequest, ModelResponse, ProviderError};
use aporia_store::Json;
use native_tls::{Certificate, TlsConnector};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

/// The total attempts one request may make, including the first.
const MAX_ATTEMPTS: u32 = 3;
/// The base backoff between attempts; attempt n waits n times this.
const BACKOFF_MS: u64 = 200;

/// A client for an OpenAI-compatible chat-completions endpoint over TLS.
pub struct HttpsProvider {
    host: String,
    port: u16,
    path: String,
    model: String,
    timeout: Duration,
    label: String,
    api_key: Option<String>,
    connector: TlsConnector,
}

// The key is a credential: the debug form names the endpoint and says whether auth is present,
// never the key itself. The connector and label are omitted for the same reason the engine's
// debug form omits its internals: this is for a person reading a log line.
#[allow(clippy::missing_fields_in_debug)]
impl std::fmt::Debug for HttpsProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpsProvider")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("path", &self.path)
            .field("model", &self.model)
            .field("has_api_key", &self.api_key.is_some())
            .finish()
    }
}

impl HttpsProvider {
    /// `base` is `https://host[:port]/v1`; `/chat/completions` is appended unless already present.
    /// Verification uses the platform trust store.
    pub fn new(base: &str, model: &str, timeout_ms: u64) -> Result<Self, ProviderError> {
        if model.trim().is_empty() {
            return Err(ProviderError::Malformed(
                "the HTTPS provider needs a model name".to_string(),
            ));
        }
        if !base.starts_with("https://") {
            return Err(ProviderError::Malformed(
                "the TLS provider needs an https:// endpoint; plain HTTP has its own provider"
                    .to_string(),
            ));
        }
        let Endpoint {
            host, port, path, ..
        } = wire::parse_base(base)?;
        let connector = TlsConnector::new()
            .map_err(|error| ProviderError::Unreachable(format!("TLS init: {error}")))?;
        let timeout = Duration::from_millis(timeout_ms.max(1));
        let label = format!(
            "openai-compatible https://{host}:{port}{path} model {model} (timeout {} ms)",
            timeout.as_millis()
        );
        Ok(Self {
            host,
            port,
            path,
            model: model.to_string(),
            timeout,
            label,
            api_key: None,
            connector,
        })
    }

    /// Read the bearer key from the named environment variable. A named-but-unset variable is
    /// a refusal at construction, not a silent unauthenticated request.
    pub fn with_api_key_env(self, variable: &str) -> Result<Self, ProviderError> {
        let value = std::env::var(variable).map_err(|_| {
            ProviderError::Malformed(format!(
                "the environment variable `{variable}` does not hold an API key"
            ))
        })?;
        self.with_api_key(value).map_err(|_| {
            ProviderError::Malformed(format!("the API key in `{variable}` is unusable"))
        })
    }

    /// Set the bearer key directly, for a caller that obtained it from a secret store. The key
    /// is validated, never logged, and redacted from anything the endpoint echoes back.
    pub fn with_api_key(mut self, value: String) -> Result<Self, ProviderError> {
        if value.trim().is_empty() {
            return Err(ProviderError::Malformed("the API key is empty".to_string()));
        }
        if value.contains('\r') || value.contains('\n') {
            return Err(ProviderError::Malformed(
                "the API key contains a newline".to_string(),
            ));
        }
        self.api_key = Some(value);
        Ok(self)
    }

    /// Add a root certificate (PEM) to the trust store, for an enterprise CA or a test server.
    /// Verification stays on; this widens trust, it does not disable it.
    pub fn with_root_certificate_pem(mut self, pem: &[u8]) -> Result<Self, ProviderError> {
        let certificate = Certificate::from_pem(pem)
            .map_err(|error| ProviderError::Malformed(format!("the root certificate: {error}")))?;
        let mut builder = TlsConnector::builder();
        builder.add_root_certificate(certificate);
        self.connector = builder
            .build()
            .map_err(|error| ProviderError::Unreachable(format!("TLS init: {error}")))?;
        Ok(self)
    }

    fn connect(&self) -> Result<native_tls::TlsStream<TcpStream>, ProviderError> {
        let address = format!("{}:{}", self.host, self.port);
        let addresses = address
            .to_socket_addrs()
            .map_err(|error| ProviderError::Unreachable(format!("{address}: {error}")))?;
        let mut last = None;
        for address in addresses {
            match TcpStream::connect_timeout(&address, self.timeout) {
                Ok(stream) => {
                    stream
                        .set_read_timeout(Some(self.timeout))
                        .map_err(|error| ProviderError::Unreachable(error.to_string()))?;
                    stream
                        .set_write_timeout(Some(self.timeout))
                        .map_err(|error| ProviderError::Unreachable(error.to_string()))?;
                    return self.connector.connect(&self.host, stream).map_err(|error| {
                        ProviderError::Unreachable(format!("TLS handshake: {error}"))
                    });
                }
                Err(error) => last = Some(error),
            }
        }
        Err(ProviderError::Unreachable(format!(
            "could not connect to {address}: {}",
            last.map_or_else(|| "no address".to_string(), |error| error.to_string())
        )))
    }

    /// Whether trying again is honest: the request never got an answer, or the server asked
    /// for patience. A 4xx refusal (other than 429) is a decision, not a transient failure.
    fn retryable(error: &ProviderError) -> bool {
        match error {
            ProviderError::Unreachable(_) | ProviderError::Timeout => true,
            ProviderError::Status(code, _) => {
                matches!(code, 429 | 500 | 502 | 503 | 504)
            }
            _ => false,
        }
    }

    /// Remove the key from anything the endpoint echoed back.
    fn redact(&self, text: String) -> String {
        match &self.api_key {
            Some(key) => text.replace(key.as_str(), "[redacted]"),
            None => text,
        }
    }

    fn redact_error(&self, error: ProviderError) -> ProviderError {
        match error {
            ProviderError::Status(code, body) => ProviderError::Status(code, self.redact(body)),
            ProviderError::Malformed(reason) => ProviderError::Malformed(self.redact(reason)),
            ProviderError::Unreachable(reason) => ProviderError::Unreachable(self.redact(reason)),
            ProviderError::Limit(reason) => ProviderError::Limit(self.redact(reason)),
            other => other,
        }
    }
}

impl ModelProvider for HttpsProvider {
    fn complete(&mut self, request: &ModelRequest) -> Result<ModelResponse, ProviderError> {
        let body = wire::encode_request(&self.model, request).to_compact();
        let headers: Vec<(String, String)> = self
            .api_key
            .as_ref()
            .map(|key| vec![("Authorization".to_string(), format!("Bearer {key}"))])
            .unwrap_or_default();
        let mut attempt = 0;
        loop {
            attempt += 1;
            let result = self
                .connect()
                .and_then(|mut stream| {
                    wire::exchange(
                        &mut stream,
                        &self.host,
                        self.port,
                        &self.path,
                        &body,
                        &headers,
                    )
                })
                .and_then(|(_, text)| {
                    let document = Json::parse(&text).map_err(|error| {
                        ProviderError::Malformed(format!("the answer is not JSON: {error}"))
                    })?;
                    wire::decode_response(&document)
                });
            match result {
                Ok(response) => return Ok(response),
                Err(error) => {
                    let error = self.redact_error(error);
                    if attempt < MAX_ATTEMPTS && Self::retryable(&error) {
                        std::thread::sleep(Duration::from_millis(BACKOFF_MS * u64::from(attempt)));
                        continue;
                    }
                    return Err(error);
                }
            }
        }
    }

    fn describe(&self) -> String {
        // The label never carries the key; it says only whether authorization is configured.
        if self.api_key.is_some() {
            format!("{} (bearer auth from environment)", self.label)
        } else {
            self.label.clone()
        }
    }
}
