//! The push-`publishDiagnostics` cache.
//!
//! LSP offers two diagnostic mechanisms. **Pull** (`textDocument/diagnostic`) is
//! a request the tool makes and the server answers synchronously; **push**
//! (`textDocument/publishDiagnostics`) is a server-to-client notification with no
//! capability gate, which the client forwards to a broadcast channel. The
//! [`DiagnosticsCache`] is the consumer that captures pushed reports so the
//! diagnostics tool can read the latest per file.
//!
//! It subscribes to the client's notification broadcast **at startup**, before
//! any tool can trigger a `didOpen`, so a report published in response to
//! opening a document is never missed. Each stored entry carries a monotonic
//! revision stamp, and [`DiagnosticsCache::await_newer`] lets the tool await a
//! report newer than the one it saw before opening the document, under a bounded
//! timeout.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use lsp_types::{Diagnostic, PublishDiagnosticsParams};
use tokio::sync::{broadcast, Mutex, Notify};

use crate::lsp::message::{self, Notification};

/// The cached diagnostics for one document.
#[derive(Debug, Clone)]
pub struct CachedDiagnostics {
    /// The monotonic revision this report was stored under.
    pub revision: u64,
    /// The diagnostics the server last published for the document.
    pub diagnostics: Vec<Diagnostic>,
}

/// The mutable state shared between the cache handle and its consumer task.
#[derive(Debug, Default)]
struct Inner {
    /// The latest entry per URI, keyed by `uri.as_str()`.
    entries: Mutex<HashMap<String, CachedDiagnostics>>,
    /// A monotonic counter stamped onto every stored report.
    revision: Mutex<u64>,
    /// Woken on every publish so `await_newer` can re-check.
    published: Notify,
}

/// A cache of the latest pushed diagnostics per document URI.
#[derive(Clone)]
pub struct DiagnosticsCache {
    inner: Arc<Inner>,
}

impl DiagnosticsCache {
    /// Spawn the background consumer over `receiver` and return the cache.
    ///
    /// Call this before any `didOpen` so no publish is missed. The task ends when
    /// the sender behind `receiver` is dropped (the client owning the broadcast
    /// channel is dropped), so no explicit shutdown is needed.
    pub fn spawn(mut receiver: broadcast::Receiver<Notification>) -> Self {
        let cache = Self::empty();
        let inner = Arc::clone(&cache.inner);
        tokio::spawn(async move {
            loop {
                match receiver.recv().await {
                    Ok(notification) => {
                        if let Some(params) = decode_publish(&notification) {
                            store(&inner, params.uri.as_str().to_string(), params.diagnostics)
                                .await;
                        }
                    }
                    // Some notifications were dropped, but the stream is still
                    // usable; keep consuming.
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    // The client (and its sender) is gone.
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
        cache
    }

    /// An empty cache with no consumer task.
    fn empty() -> Self {
        Self {
            inner: Arc::new(Inner::default()),
        }
    }

    /// The current cached entry for `uri`, if any.
    pub async fn get(&self, uri: &str) -> Option<CachedDiagnostics> {
        self.inner.entries.lock().await.get(uri).cloned()
    }

    /// Await an entry for `uri` with a revision newer than `baseline`, or `None`
    /// if none arrives within `timeout`.
    pub async fn await_newer(
        &self,
        uri: &str,
        baseline: u64,
        timeout: Duration,
    ) -> Option<CachedDiagnostics> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            // Register for the next wake-up *before* checking, so a publish
            // between the check and the await cannot be missed.
            let notified = self.inner.published.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some(entry) = self.get(uri).await {
                if entry.revision > baseline {
                    return Some(entry);
                }
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return None;
            }
        }
    }
}

/// Store a decoded report under a fresh revision and wake waiters.
async fn store(inner: &Arc<Inner>, uri: String, diagnostics: Vec<Diagnostic>) {
    let mut revision = inner.revision.lock().await;
    *revision += 1;
    let stamp = *revision;
    inner.entries.lock().await.insert(
        uri,
        CachedDiagnostics {
            revision: stamp,
            diagnostics,
        },
    );
    inner.published.notify_waiters();
}

/// Decode a server notification into `publishDiagnostics` params, if it is one.
fn decode_publish(notification: &Notification) -> Option<PublishDiagnosticsParams> {
    if notification.method != message::methods::PUBLISH_DIAGNOSTICS {
        return None;
    }
    let params = notification.params.clone()?;
    serde_json::from_value(params).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{Position, Range};

    fn cache() -> DiagnosticsCache {
        DiagnosticsCache::empty()
    }

    fn diagnostic(message: &str) -> Diagnostic {
        Diagnostic {
            range: Range::new(Position::new(0, 0), Position::new(0, 1)),
            message: message.to_string(),
            ..Default::default()
        }
    }

    async fn put(cache: &DiagnosticsCache, uri: &str, messages: &[&str]) {
        store(
            &cache.inner,
            uri.to_string(),
            messages.iter().map(|message| diagnostic(message)).collect(),
        )
        .await;
    }

    #[tokio::test]
    async fn stores_and_replaces_per_uri() {
        let cache = cache();
        put(&cache, "file:///a", &["first"]).await;
        let first = cache.get("file:///a").await.unwrap();
        assert_eq!(first.diagnostics.len(), 1);
        assert_eq!(first.diagnostics[0].message, "first");

        put(&cache, "file:///a", &["one", "two"]).await;
        let second = cache.get("file:///a").await.unwrap();
        assert_eq!(second.diagnostics.len(), 2);
        assert!(second.revision > first.revision);
    }

    #[tokio::test]
    async fn await_newer_returns_an_already_cached_report() {
        let cache = cache();
        put(&cache, "file:///a", &["cached"]).await;
        let entry = cache
            .await_newer("file:///a", 0, Duration::from_millis(50))
            .await
            .unwrap();
        assert_eq!(entry.diagnostics[0].message, "cached");
    }

    #[tokio::test]
    async fn await_newer_returns_none_when_the_baseline_is_current() {
        let cache = cache();
        put(&cache, "file:///a", &["only"]).await;
        let baseline = cache.get("file:///a").await.unwrap().revision;
        assert!(cache
            .await_newer("file:///a", baseline, Duration::from_millis(30))
            .await
            .is_none());
    }

    #[tokio::test]
    async fn await_newer_sees_a_report_published_after_the_call() {
        let cache = cache();
        let publisher = cache.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            put(&publisher, "file:///a", &["fresh"]).await;
        });
        let entry = cache
            .await_newer("file:///a", 0, Duration::from_millis(500))
            .await
            .unwrap();
        assert_eq!(entry.diagnostics[0].message, "fresh");
    }

    #[test]
    fn decodes_a_publish_notification() {
        let notification = Notification {
            jsonrpc: message::JSONRPC_VERSION.to_string(),
            method: message::methods::PUBLISH_DIAGNOSTICS.to_string(),
            params: Some(serde_json::json!({
                "uri": "file:///a",
                "diagnostics": [
                    {"range": {"start": {"line": 0, "character": 0},
                               "end": {"line": 0, "character": 1}},
                     "message": "boom", "severity": 1}
                ]
            })),
        };
        let decoded = decode_publish(&notification).unwrap();
        assert_eq!(decoded.uri.as_str(), "file:///a");
        assert_eq!(decoded.diagnostics.len(), 1);
        assert_eq!(decoded.diagnostics[0].message, "boom");
    }

    #[test]
    fn a_non_publish_notification_decodes_to_none() {
        let notification = Notification {
            jsonrpc: message::JSONRPC_VERSION.to_string(),
            method: "textDocument/didSave".to_string(),
            params: None,
        };
        assert!(decode_publish(&notification).is_none());
    }
}
