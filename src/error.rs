//! Typed errors for the adapter.
//!
//! Every failure the LSP client can surface has a dedicated variant so the tool
//! layer can turn it into a clean, actionable message. `anyhow` is used only at
//! the `main` boundary to attach context; nothing inside the adapter depends on
//! it.

use std::time::Duration;

use thiserror::Error;

/// The adapter's result type.
pub type Result<T> = std::result::Result<T, Error>;

/// A failure while launching, talking to, or shutting down an LSP server.
#[derive(Debug, Error)]
pub enum Error {
    /// The server process could not be launched.
    #[error("failed to spawn LSP server `{command}`: {source}")]
    SpawnFailed {
        /// The command that was attempted.
        command: String,
        /// The underlying I/O error from the runtime.
        #[source]
        source: std::io::Error,
    },

    /// An I/O error on the transport.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// A `Content-Length` framing error.
    #[error("framing error: {0}")]
    Framing(String),

    /// A protocol-level error (a malformed or unexpected message).
    #[error("protocol error: {0}")]
    Protocol(String),

    /// A JSON (de)serialization error.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// The server answered a request with a JSON-RPC error object.
    #[error("LSP error response {code}: {message}")]
    JsonRpc {
        /// The JSON-RPC error code.
        code: i64,
        /// The human-readable error message.
        message: String,
    },

    /// A request did not receive a response within its timeout.
    #[error("request timed out after {0:?}")]
    Timeout(Duration),

    /// The server exited (or its stdio closed) before the operation finished.
    #[error("LSP server exited unexpectedly")]
    ServerExited,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_failed_names_the_command_and_cause() {
        let err = Error::SpawnFailed {
            command: "definitely-not-a-real-server".to_string(),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "No such file or directory"),
        };
        let message = err.to_string();
        assert!(message.contains("definitely-not-a-real-server"));
        assert!(message.contains("No such file or directory"));
    }

    #[test]
    fn framing_error_is_rendered() {
        let err = Error::Framing("missing Content-Length header".to_string());
        assert_eq!(
            err.to_string(),
            "framing error: missing Content-Length header"
        );
    }

    #[test]
    fn timeout_reports_the_duration() {
        let err = Error::Timeout(Duration::from_millis(1500));
        assert!(err.to_string().contains("1.5s"), "got: {err}");
    }

    #[test]
    fn server_exited_is_rendered() {
        assert_eq!(
            Error::ServerExited.to_string(),
            "LSP server exited unexpectedly"
        );
    }

    #[test]
    fn json_rpc_error_carries_code_and_message() {
        let err = Error::JsonRpc {
            code: -32601,
            message: "method not found".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "LSP error response -32601: method not found"
        );
    }

    #[test]
    fn io_errors_convert_with_the_from_impl() {
        let err: Error = std::io::Error::new(std::io::ErrorKind::BrokenPipe, "broken pipe").into();
        assert!(matches!(err, Error::Io(_)));
    }
}
