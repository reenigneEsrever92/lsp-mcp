//! The MCP server: the tool router and the stdio transport.
//!
//! [`LspServer`] owns the shared [`App`](crate::app::App) and a
//! [`ToolRouter`]. The `#[tool]` methods stay thin — they parse the shared
//! argument type and delegate to [`crate::tools`] — and `get_info` advertises
//! tool support. [`serve`] builds the newline-delimited stdio transport and
//! waits for it to close.

use std::sync::Arc;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ServerCapabilities, ServerConfig};
use rmcp::{tool, tool_handler, tool_router, ServerHandler, ServiceExt};

use crate::app::App;
use crate::tools::{
    ApplyCodeActionArgs, CodeActionArgs, FileArgs, FormatArgs, PositionArgs, ReferencesArgs,
    RenameArgs, WorkspaceSymbolArgs,
};

/// The MCP server exposing the LSP adapter's tools.
#[derive(Clone)]
pub struct LspServer {
    app: Arc<App>,
    tool_router: ToolRouter<Self>,
}

impl LspServer {
    /// Build a server over the shared application state.
    pub fn new(app: Arc<App>) -> Self {
        Self {
            app,
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router]
impl LspServer {
    /// Locate the definition of the symbol at a position.
    #[tool(
        name = "definition",
        description = "Locate the definition of the symbol at a 1-based line and column, returning each location's URI, position, surrounding code context, and the raw LSP payload."
    )]
    async fn definition(
        &self,
        Parameters(args): Parameters<PositionArgs>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        crate::tools::definition::run(Arc::clone(&self.app), args).await
    }

    /// List the references to the symbol at a position.
    #[tool(
        name = "references",
        description = "List the references to the symbol at a 1-based line and column, returning each location's URI, position, surrounding code context, and the raw LSP payload. Controls whether the declaration itself is included."
    )]
    async fn references(
        &self,
        Parameters(args): Parameters<ReferencesArgs>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        crate::tools::references::run(Arc::clone(&self.app), args).await
    }

    /// Show the hover (type, signature, documentation) at a position.
    #[tool(
        name = "hover",
        description = "Show the hover information (type, signature, documentation) for the symbol at a 1-based line and column, returning the rendered contents, the optional range, surrounding code context, and the raw LSP payload."
    )]
    async fn hover(
        &self,
        Parameters(args): Parameters<PositionArgs>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        crate::tools::hover::run(Arc::clone(&self.app), args).await
    }

    /// List the symbols in a file.
    #[tool(
        name = "documentSymbol",
        description = "List the symbols in a file, returning each symbol's name, kind, selection position, surrounding code context, and the raw LSP payload."
    )]
    async fn document_symbol(
        &self,
        Parameters(args): Parameters<FileArgs>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        crate::tools::document_symbol::run(Arc::clone(&self.app), args).await
    }

    /// Search the workspace for symbols matching a query.
    #[tool(
        name = "workspaceSymbol",
        description = "Search the workspace for symbols matching a query, returning each match's name, kind, file URI, position, surrounding code context where available, and the raw LSP payload."
    )]
    async fn workspace_symbol(
        &self,
        Parameters(args): Parameters<WorkspaceSymbolArgs>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        crate::tools::workspace_symbol::run(Arc::clone(&self.app), args).await
    }

    /// Report a file's diagnostics (pull or push).
    #[tool(
        name = "diagnostics",
        description = "Report a file's diagnostics, preferring the pull mechanism (textDocument/diagnostic) when the server supports it and otherwise reading pushed textDocument/publishDiagnostics reports. Returns each diagnostic's severity, code, source, message, 1-based position, surrounding code context, the mechanism used, and the raw LSP payload."
    )]
    async fn diagnostics(
        &self,
        Parameters(args): Parameters<FileArgs>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        crate::tools::diagnostics::run(Arc::clone(&self.app), args).await
    }

    /// List the code actions available at a position.
    #[tool(
        name = "codeAction",
        description = "List the code actions available at a 1-based line and column (optionally scoped to the diagnostics there and filtered by kind), returning each action's index, title, kind, flags, and the raw LSP payload. Read-only: use `applyCodeAction` to apply one."
    )]
    async fn code_action(
        &self,
        Parameters(args): Parameters<CodeActionArgs>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        crate::tools::code_action::run(Arc::clone(&self.app), args).await
    }

    /// Apply a chosen code action at a position.
    #[tool(
        name = "applyCodeAction",
        description = "Apply the code action at `index` from a fresh list fetched with the same arguments as `codeAction` (optionally guarding on the action's `title`), resolving it via codeAction/resolve when needed and applying its WorkspaceEdit to disk and/or executing its command. Reports the files changed/created/renamed/deleted and the executed command."
    )]
    async fn apply_code_action(
        &self,
        Parameters(args): Parameters<ApplyCodeActionArgs>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        crate::tools::apply_code_action::run(Arc::clone(&self.app), args).await
    }

    /// Rename the symbol at a position across every file that uses it.
    #[tool(
        name = "rename",
        description = "Rename the symbol at a 1-based line and column to `new_name`, validating the position with textDocument/prepareRename first, then applying the WorkspaceEdit returned by textDocument/rename to disk. Reports the old and new names and the files changed."
    )]
    async fn rename(
        &self,
        Parameters(args): Parameters<RenameArgs>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        crate::tools::rename::run(Arc::clone(&self.app), args).await
    }

    /// Format a whole file or a range within it.
    #[tool(
        name = "format",
        description = "Format a file's contents through textDocument/formatting, or a single range through textDocument/rangeFormatting when a 1-based `range` is given, applying the returned edits to disk. Reports the file formatted and the edits applied."
    )]
    async fn format(
        &self,
        Parameters(args): Parameters<FormatArgs>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        crate::tools::format::run(Arc::clone(&self.app), args).await
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for LspServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
            "Inspect and modify a language server's view of a codebase. Read/explore tools: `definition` locates a symbol's definition; `references` lists references to a symbol; `hover` shows a symbol's type, signature, and documentation; `documentSymbol` lists a file's symbols; `workspaceSymbol` searches the workspace for symbols; `diagnostics` reports a file's diagnostics (pull or push). Act tools: `codeAction` lists the code actions available at a position, and `applyCodeAction` applies a chosen one (identified by its `index`), writing any resulting edits to disk and/or running its command. Write tools: `rename` renames the symbol at a position everywhere it is used via textDocument/prepareRename then textDocument/rename, and `format` formats a whole file or a range. Position-based tools take a file and a 1-based line and column.",
        )
    }
}

/// Serve the MCP protocol over stdio until the transport closes.
pub async fn serve(app: Arc<App>) -> anyhow::Result<()> {
    let running = LspServer::new(app).serve(rmcp::transport::stdio()).await?;
    running.waiting().await?;
    Ok(())
}
