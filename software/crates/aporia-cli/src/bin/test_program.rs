//! The program `aporia-cli`'s integration tests spawn, and the smallest possible worked example of
//! the external-program protocol: two lines, because the loop lives in
//! `aporia_adapter::example`.
//!
//! It lives here rather than being pointed at from `aporia-adapter` for a mundane reason — Cargo only
//! gives a test binary the `CARGO_BIN_EXE_*` of bins in its own package — and the thing it proves is
//! not mundane: the CLI's tests drive a genuine second process over real pipes, so the flag handling,
//! the provenance line and the exit statuses are all exercised against a program that knows nothing
//! about APORIA's test suite.
//!
//! `APORIA_PROGRAM_MODE` selects its behaviour the same way it does for the adapter's example: `answer`
//! (default), `exit`, `garbage`, `banner`, `arity`, `refuse`, `nan`, `hang`.

fn main() {
    aporia_adapter::example::serve();
}
