//! An OpenAI-compatible endpoint over plain HTTP, with no TLS.
//!
//! The first slice runs a model locally: llama.cpp, Ollama's `OpenAI` compatibility, LM Studio and
//! friends all speak this shape. The client is deliberately small and strict -- one request, one
//! `Content-Length` response, no chunked transfer, no TLS. An HTTPS endpoint belongs behind the
//! same trait with the TLS-capable provider ([`super::https`]), not a half-supported one here.

use super::wire::{self, Endpoint};
use super::{ModelProvider, ModelRequest, ModelResponse, ProviderError};
use aporia_store::Json;
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

/// A client for a local OpenAI-compatible chat-completions endpoint.
#[derive(Debug)]
pub struct HttpProvider {
    host: String,
    port: u16,
    path: String,
    model: String,
    timeout: Duration,
    label: String,
}

impl HttpProvider {
    /// `base` is `http://host[:port]/v1`; `/chat/completions` is appended unless already present.
    pub fn new(base: &str, model: &str, timeout_ms: u64) -> Result<Self, ProviderError> {
        if model.trim().is_empty() {
            return Err(ProviderError::Malformed(
                "the HTTP provider needs a model name".to_string(),
            ));
        }
        if base.starts_with("https://") {
            return Err(ProviderError::Malformed(
                "this provider speaks plain HTTP only; an HTTPS endpoint needs the TLS provider"
                    .to_string(),
            ));
        }
        let Endpoint {
            host, port, path, ..
        } = wire::parse_base(base)?;
        let timeout = Duration::from_millis(timeout_ms.max(1));
        let label = format!(
            "openai-compatible http://{host}:{port}{path} model {model} (timeout {} ms)",
            timeout.as_millis()
        );
        Ok(Self {
            host,
            port,
            path,
            model: model.to_string(),
            timeout,
            label,
        })
    }

    fn connect(&self) -> Result<TcpStream, ProviderError> {
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
                    return Ok(stream);
                }
                Err(error) => last = Some(error),
            }
        }
        Err(ProviderError::Unreachable(format!(
            "could not connect to {address}: {}",
            last.map_or_else(|| "no address".to_string(), |error| error.to_string())
        )))
    }
}

impl ModelProvider for HttpProvider {
    fn complete(&mut self, request: &ModelRequest) -> Result<ModelResponse, ProviderError> {
        let body = wire::encode_request(&self.model, request).to_compact();
        let mut stream = self.connect()?;
        let (_, text) = wire::exchange(&mut stream, &self.host, self.port, &self.path, &body, &[])?;
        let document = Json::parse(&text).map_err(|error| {
            ProviderError::Malformed(format!("the answer is not JSON: {error}"))
        })?;
        wire::decode_response(&document)
    }

    fn describe(&self) -> String {
        self.label.clone()
    }
}
