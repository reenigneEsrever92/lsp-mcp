//! The shared application state.
//!
//! One [`Client`] is created at startup and handed to [`App`], which every tool
//! and the MCP server share behind an `Arc`. `Client::request` takes `&self`, so
//! the client sits behind an `Arc`; [`Documents`] takes `&mut self`, so the
//! on-demand document store sits behind a `tokio::sync::Mutex` (the tools are
//! async). This is the single boundary the tool layer is wired through.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use lsp_types::ServerCapabilities;
use tokio::process::{ChildStdin, ChildStdout};
use tokio::sync::Mutex;

use crate::diagnostics::DiagnosticsCache;
use crate::error::{Error, Result};
use crate::lsp::documents::Documents;
use crate::lsp::Client;

/// The state shared by the MCP server and every tool.
pub struct App {
    client: Arc<Client<ChildStdout, ChildStdin>>,
    documents: Mutex<Documents>,
    diagnostics: Arc<DiagnosticsCache>,
}

impl App {
    /// Build the shared state around the already-spawned LSP client.
    ///
    /// The push-diagnostics cache is created here, subscribing to the client's
    /// notification broadcast **before** any tool can trigger a `didOpen`, so no
    /// `publishDiagnostics` report is missed.
    pub fn new(client: Arc<Client<ChildStdout, ChildStdin>>) -> Self {
        let diagnostics = Arc::new(DiagnosticsCache::spawn(client.subscribe()));
        Self {
            client,
            documents: Mutex::new(Documents::new()),
            diagnostics,
        }
    }

    /// The LSP client.
    pub fn client(&self) -> &Client<ChildStdout, ChildStdin> {
        &self.client
    }

    /// The push-`publishDiagnostics` cache.
    pub fn diagnostics(&self) -> &DiagnosticsCache {
        &self.diagnostics
    }

    /// The server capabilities captured during the handshake, if any.
    pub fn capabilities(&self) -> Option<ServerCapabilities> {
        self.client.capabilities()
    }

    /// Whether the server advertises `textDocument/definition`.
    pub fn supports_definition(&self) -> bool {
        advertises(&self.capabilities(), |capabilities| {
            capabilities.definition_provider.is_some()
        })
    }

    /// Whether the server advertises `textDocument/references`.
    pub fn supports_references(&self) -> bool {
        advertises(&self.capabilities(), |capabilities| {
            capabilities.references_provider.is_some()
        })
    }

    /// Whether the server advertises `textDocument/hover`.
    pub fn supports_hover(&self) -> bool {
        advertises(&self.capabilities(), |capabilities| {
            capabilities.hover_provider.is_some()
        })
    }

    /// Whether the server advertises `textDocument/documentSymbol`.
    pub fn supports_document_symbols(&self) -> bool {
        advertises(&self.capabilities(), |capabilities| {
            capabilities.document_symbol_provider.is_some()
        })
    }

    /// Whether the server advertises `workspace/symbol`.
    pub fn supports_workspace_symbols(&self) -> bool {
        advertises(&self.capabilities(), |capabilities| {
            capabilities.workspace_symbol_provider.is_some()
        })
    }

    /// Whether the server advertises pull diagnostics (`textDocument/diagnostic`).
    pub fn supports_pull_diagnostics(&self) -> bool {
        advertises(&self.capabilities(), |capabilities| {
            capabilities.diagnostic_provider.is_some()
        })
    }

    /// Whether the server advertises code actions (`textDocument/codeAction`).
    pub fn supports_code_actions(&self) -> bool {
        advertises(&self.capabilities(), |capabilities| {
            capabilities.code_action_provider.is_some()
        })
    }

    /// Whether the server advertises command execution (`workspace/executeCommand`).
    pub fn supports_execute_command(&self) -> bool {
        advertises(&self.capabilities(), |capabilities| {
            capabilities.execute_command_provider.is_some()
        })
    }

    /// Whether the server advertises rename (`textDocument/rename`).
    pub fn supports_rename(&self) -> bool {
        advertises(&self.capabilities(), |capabilities| {
            capabilities.rename_provider.is_some()
        })
    }

    /// Whether the server advertises whole-file formatting
    /// (`textDocument/formatting`).
    pub fn supports_document_formatting(&self) -> bool {
        advertises(&self.capabilities(), |capabilities| {
            capabilities.document_formatting_provider.is_some()
        })
    }

    /// Whether the server advertises range formatting
    /// (`textDocument/rangeFormatting`).
    pub fn supports_document_range_formatting(&self) -> bool {
        advertises(&self.capabilities(), |capabilities| {
            capabilities.document_range_formatting_provider.is_some()
        })
    }

    /// Read `path` from disk, ensure the server has it open, and return its text.
    ///
    /// The text is returned so a tool can validate a position against it and
    /// render code context without reading the file a second time.
    pub async fn open_document(&self, path: &Path) -> Result<String> {
        let text = std::fs::read_to_string(path).map_err(|source| {
            Error::Protocol(format!("cannot read {}: {source}", path.display()))
        })?;
        let mut documents = self.documents.lock().await;
        documents.ensure_open(&self.client, path, &text)?;
        Ok(text)
    }

    /// Best-effort refresh of `paths` after a write, so the adapter's document
    /// store and the server's view match disk again.
    ///
    /// Each path is re-read and re-opened through [`App::open_document`], which
    /// emits `didChange` when the text differs from what the server was last
    /// told. A path that no longer exists (a rename's old path, a delete) is
    /// skipped, and a failure is logged rather than returned, because the write
    /// has already succeeded.
    pub async fn refresh_documents(&self, paths: &[PathBuf]) {
        for path in paths {
            if !path.exists() {
                continue;
            }
            if let Err(error) = self.open_document(path).await {
                tracing::warn!(
                    path = %path.display(),
                    %error,
                    "failed to refresh document after a write"
                );
            }
        }
    }
}

/// Whether `capabilities` are present and `field` selects an advertised feature.
fn advertises(
    capabilities: &Option<ServerCapabilities>,
    field: impl Fn(&ServerCapabilities) -> bool,
) -> bool {
    capabilities.as_ref().map(field).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::OneOf;

    #[test]
    fn capability_predicates_read_the_advertised_flags() {
        assert!(!advertises(&None, |c| c.references_provider.is_some()));

        let mut capabilities = ServerCapabilities::default();
        assert!(!advertises(&Some(capabilities.clone()), |c| {
            c.references_provider.is_some()
        }));

        capabilities.references_provider = Some(OneOf::Left(true));
        capabilities.hover_provider = Some(lsp_types::HoverProviderCapability::Simple(true));
        capabilities.document_symbol_provider = Some(OneOf::Left(true));
        capabilities.workspace_symbol_provider = Some(OneOf::Left(true));
        capabilities.diagnostic_provider = Some(lsp_types::DiagnosticServerCapabilities::Options(
            Default::default(),
        ));
        capabilities.code_action_provider = Some(lsp_types::CodeActionProviderCapability::Options(
            Default::default(),
        ));
        capabilities.execute_command_provider = Some(lsp_types::ExecuteCommandOptions::default());
        capabilities.rename_provider = Some(OneOf::Left(true));
        capabilities.document_formatting_provider = Some(OneOf::Left(true));
        capabilities.document_range_formatting_provider = Some(OneOf::Left(true));
        let capabilities = Some(capabilities);

        assert!(advertises(&capabilities, |c| c
            .references_provider
            .is_some()));
        assert!(advertises(&capabilities, |c| c.hover_provider.is_some()));
        assert!(advertises(&capabilities, |c| {
            c.document_symbol_provider.is_some()
        }));
        assert!(advertises(&capabilities, |c| {
            c.workspace_symbol_provider.is_some()
        }));
        assert!(advertises(&capabilities, |c| {
            c.diagnostic_provider.is_some()
        }));
        assert!(advertises(&capabilities, |c| c
            .code_action_provider
            .is_some()));
        assert!(advertises(&capabilities, |c| c
            .execute_command_provider
            .is_some()));
        assert!(advertises(&capabilities, |c| c.rename_provider.is_some()));
        assert!(advertises(&capabilities, |c| c
            .document_formatting_provider
            .is_some()));
        assert!(advertises(&capabilities, |c| c
            .document_range_formatting_provider
            .is_some()));
    }

    #[test]
    fn code_action_predicates_are_false_without_capabilities() {
        assert!(!advertises(&None, |c| c.code_action_provider.is_some()));
        assert!(!advertises(&None, |c| c.execute_command_provider.is_some()));
        let capabilities = Some(ServerCapabilities::default());
        assert!(!advertises(&capabilities, |c| c
            .code_action_provider
            .is_some()));
        assert!(!advertises(&capabilities, |c| c
            .execute_command_provider
            .is_some()));
    }
}
