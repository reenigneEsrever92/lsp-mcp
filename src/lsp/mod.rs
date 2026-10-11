//! The LSP client: message framing, the JSON-RPC envelope, the session client,
//! and the on-demand document store.
//!
//! Nothing in this module knows about MCP; it is a reusable, server-agnostic
//! LSP client. The boundaries are:
//!
//! - [`transport`] — `Content-Length` framing over any `AsyncRead`/`AsyncWrite`,
//!   plus the `tokio::process` spawn constructor.
//! - [`message`] — the JSON-RPC 2.0 envelope and `lsp_types`-derived builders.
//! - [`client`] — [`Client`]: handshake, correlation, timeouts, and exit
//!   detection.
//! - [`documents`] — [`Documents`]: per-URI version and text bookkeeping.
//!
//! This module is the adapter's reusable LSP surface. The current binary drives
//! only the handshake, so parts of it are exercised by tests and reserved for the
//! MCP server and tool layer that later change requests add; `dead_code` is
//! allowed here for that reason.
#![allow(dead_code)]

pub mod client;
pub mod documents;
pub mod message;
pub mod transport;

pub use client::Client;

use std::path::{Path, PathBuf};
use std::str::FromStr;

use lsp_types::Uri;

use crate::error::{Error, Result};

/// Convert a filesystem path to an `lsp_types::Uri`.
///
/// `lsp-types` 0.97 represents document locations as [`Uri`] (backed by
/// `fluent-uri`), so `url::Url` performs the path conversion and the resulting
/// string is adapted to a [`Uri`] at this boundary.
pub fn path_to_uri(path: &Path) -> Result<Uri> {
    let url = url::Url::from_file_path(path).map_err(|()| {
        Error::Protocol(format!(
            "cannot convert path to a file URI: {}",
            path.display()
        ))
    })?;
    Uri::from_str(url.as_str())
        .map_err(|err| Error::Protocol(format!("invalid file URI {}: {err}", url.as_str())))
}

/// Convert an `lsp_types::Uri` back to a filesystem path.
pub fn uri_to_path(uri: &Uri) -> Result<PathBuf> {
    let url = url::Url::parse(uri.as_str())
        .map_err(|err| Error::Protocol(format!("invalid URI {}: {err}", uri.as_str())))?;
    url.to_file_path()
        .map_err(|()| Error::Protocol(format!("URI is not a file path: {}", uri.as_str())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_round_trips_through_a_uri() {
        let path = PathBuf::from("/tmp/lsp-mcp/example.rs");
        let uri = path_to_uri(&path).unwrap();
        assert!(uri.as_str().starts_with("file://"));
        assert_eq!(uri_to_path(&uri).unwrap(), path);
    }

    #[test]
    fn non_file_uri_is_rejected() {
        let uri = Uri::from_str("https://example.com/x.rs").unwrap();
        assert!(uri_to_path(&uri).is_err());
    }
}
