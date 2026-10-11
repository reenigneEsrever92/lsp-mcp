//! On-demand document lifecycle.
//!
//! Position-based requests are only meaningful once the server knows the
//! document, so [`Documents`] keeps each open file's version and text and emits
//! `textDocument/didOpen`, `textDocument/didChange`, and `textDocument/didClose`
//! through a [`Client`]. Changes are sent as full-document replacements, which
//! the protocol defines a server to accept whether it advertises full or
//! incremental synchronization.

use std::collections::HashMap;
use std::path::Path;

use lsp_types::Uri;

use super::client::Client;
use super::message::{self, methods};
use crate::error::Result;
use crate::lsp::path_to_uri;

/// The bookkeeping for one open document.
#[derive(Debug, Clone)]
struct OpenDocument {
    uri: Uri,
    version: i32,
    text: String,
}

/// Tracks the documents the server has been told about.
#[derive(Debug, Default)]
pub struct Documents {
    open: HashMap<String, OpenDocument>,
}

impl Documents {
    /// Create an empty document store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the document at `uri` is currently open.
    pub fn is_open(&self, uri: &Uri) -> bool {
        self.open.contains_key(uri.as_str())
    }

    /// The current version of the document at `uri`, if it is open.
    pub fn version(&self, uri: &Uri) -> Option<i32> {
        self.open.get(uri.as_str()).map(|document| document.version)
    }

    /// Ensure the document at `path` is open with `text`, emitting `didOpen` the
    /// first time and a `didChange` only when the text actually differs.
    pub fn ensure_open<R, W>(
        &mut self,
        client: &Client<R, W>,
        path: &Path,
        text: &str,
    ) -> Result<()> {
        let uri = path_to_uri(path)?;
        match self.open.get_mut(uri.as_str()) {
            Some(document) => {
                if document.text != text {
                    document.version += 1;
                    document.text = text.to_string();
                    let params = message::did_change_params(&uri, document.version, text.to_string());
                    client.notify(methods::DID_CHANGE, Some(serde_json::to_value(params)?))?;
                }
                Ok(())
            }
            None => {
                let version = 1;
                self.open.insert(
                    uri.as_str().to_string(),
                    OpenDocument {
                        uri: uri.clone(),
                        version,
                        text: text.to_string(),
                    },
                );
                let params = message::did_open_params(
                    &uri,
                    language_id(path).to_string(),
                    version,
                    text.to_string(),
                );
                client.notify(methods::DID_OPEN, Some(serde_json::to_value(params)?))
            }
        }
    }

    /// Close the document at `path`, emitting `didClose` if it was open.
    pub fn close<R, W>(&mut self, client: &Client<R, W>, path: &Path) -> Result<()> {
        let uri = path_to_uri(path)?;
        if self.open.remove(uri.as_str()).is_some() {
            let params = message::did_close_params(&uri);
            client.notify(methods::DID_CLOSE, Some(serde_json::to_value(params)?))?;
        }
        Ok(())
    }
}

/// A best-effort LSP language id from a file extension.
fn language_id(path: &Path) -> &'static str {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("rs") => "rust",
        Some("ts") => "typescript",
        Some("tsx") => "typescriptreact",
        Some("js") => "javascript",
        Some("jsx") => "javascriptreact",
        Some("py") => "python",
        Some("go") => "go",
        Some("java") => "java",
        Some("c") => "c",
        Some("h") => "c",
        Some("cc") | Some("cpp") | Some("hpp") => "cpp",
        Some("json") => "json",
        Some("md") => "markdown",
        _ => "plaintext",
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::Value;
    use tokio::io::DuplexStream;
    use tokio::sync::mpsc;

    use super::*;
    use crate::lsp::transport;

    const BUFFER: usize = 16 * 1024;

    /// A client whose writer is drained into a channel, so tests can inspect the
    /// exact notifications the document store emits.
    fn capturing_client() -> (
        Client<DuplexStream, DuplexStream>,
        mpsc::UnboundedReceiver<Value>,
    ) {
        let (client_writer, mut server_reader) = tokio::io::duplex(BUFFER);
        let (_server_writer, client_reader) = tokio::io::duplex(BUFFER);
        let (sender, receiver) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Ok(Some(frame)) = transport::read_frame(&mut server_reader).await {
                if let Ok(value) = serde_json::from_slice::<Value>(&frame) {
                    if sender.send(value).is_err() {
                        break;
                    }
                }
            }
        });
        (
            Client::new(client_reader, client_writer, Duration::from_secs(2)),
            receiver,
        )
    }

    async fn next(receiver: &mut mpsc::UnboundedReceiver<Value>) -> Value {
        tokio::time::timeout(Duration::from_secs(2), receiver.recv())
            .await
            .expect("a notification")
            .expect("the channel to be open")
    }

    #[tokio::test]
    async fn opening_tracks_version_and_sends_full_text() {
        let (client, mut received) = capturing_client();
        let mut documents = Documents::new();
        let path = Path::new("/tmp/lsp-mcp-tests/doc.rs");

        documents.ensure_open(&client, path, "fn main() {}").unwrap();

        let uri = path_to_uri(path).unwrap();
        assert!(documents.is_open(&uri));
        assert_eq!(documents.version(&uri), Some(1));

        let notification = next(&mut received).await;
        assert_eq!(notification["method"], "textDocument/didOpen");
        assert_eq!(notification["params"]["textDocument"]["languageId"], "rust");
        assert_eq!(notification["params"]["textDocument"]["version"], 1);
        assert_eq!(notification["params"]["textDocument"]["text"], "fn main() {}");
    }

    #[tokio::test]
    async fn reopening_unchanged_text_is_a_no_op() {
        let (client, mut received) = capturing_client();
        let mut documents = Documents::new();
        let path = Path::new("/tmp/lsp-mcp-tests/doc.rs");

        documents.ensure_open(&client, path, "same").unwrap();
        let _ = next(&mut received).await;
        documents.ensure_open(&client, path, "same").unwrap();

        assert!(
            tokio::time::timeout(Duration::from_millis(100), received.recv())
                .await
                .is_err(),
            "no notification expected for unchanged text"
        );
        assert_eq!(documents.version(&path_to_uri(path).unwrap()), Some(1));
    }

    #[tokio::test]
    async fn changing_text_increments_the_version_and_sends_full_text() {
        let (client, mut received) = capturing_client();
        let mut documents = Documents::new();
        let path = Path::new("/tmp/lsp-mcp-tests/doc.rs");

        documents.ensure_open(&client, path, "one").unwrap();
        let _ = next(&mut received).await;

        documents.ensure_open(&client, path, "two").unwrap();
        let notification = next(&mut received).await;

        let uri = path_to_uri(path).unwrap();
        assert_eq!(documents.version(&uri), Some(2));
        assert_eq!(notification["method"], "textDocument/didChange");
        assert_eq!(notification["params"]["textDocument"]["version"], 2);
        assert_eq!(notification["params"]["contentChanges"][0]["text"], "two");
        assert!(notification["params"]["contentChanges"][0]["range"].is_null());
    }

    #[tokio::test]
    async fn closing_removes_the_document_and_notifies() {
        let (client, mut received) = capturing_client();
        let mut documents = Documents::new();
        let path = Path::new("/tmp/lsp-mcp-tests/doc.rs");

        documents.ensure_open(&client, path, "text").unwrap();
        let _ = next(&mut received).await;

        documents.close(&client, path).unwrap();
        let notification = next(&mut received).await;

        let uri = path_to_uri(path).unwrap();
        assert!(!documents.is_open(&uri));
        assert_eq!(notification["method"], "textDocument/didClose");
        assert_eq!(notification["params"]["textDocument"]["uri"], uri.as_str());

        // Closing again is a no-op.
        documents.close(&client, path).unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(100), received.recv())
                .await
                .is_err()
        );
    }
}
