//! `lsp-mcp` — launch an LSP server and expose it to an MCP client over stdio.
//!
//! This entry point is the only place that wires the process lifecycle to the
//! MCP server. It initializes diagnostics to **stderr** (stdout is reserved for
//! the MCP transport), parses the CLI, spawns the LSP client (the handshake runs
//! inside [`lsp::Client::spawn`]), builds the shared [`app::App`], and serves MCP
//! over stdio until the transport closes. On an error it prints to stderr and
//! exits non-zero, without a panic.

mod app;
mod cli;
mod diagnostics;
mod error;
mod lsp;
mod mcp;
mod tools;
mod workspace_edit;

use std::process::ExitCode;
use std::sync::Arc;

use anyhow::Context;
use clap::Parser;

use crate::app::App;
use crate::cli::Cli;

#[tokio::main]
async fn main() -> ExitCode {
    init_tracing();
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("lsp-mcp: {error:#}");
            ExitCode::FAILURE
        }
    }
}

/// Initialize logging, writing everything to stderr so stdout stays clean.
fn init_tracing() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_max_level(tracing::Level::INFO)
        .init();
}

async fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let command = cli.command().to_string();
    let args = cli.args().to_vec();

    tracing::info!(server = %command, "starting LSP server");
    let client = lsp::Client::spawn(&command, &args)
        .await
        .with_context(|| format!("failed to start LSP server `{command}`"))?;

    let capabilities = client.capabilities().unwrap_or_default();
    tracing::info!(
        definition = capabilities.definition_provider.is_some(),
        hover = capabilities.hover_provider.is_some(),
        references = capabilities.references_provider.is_some(),
        document_symbols = capabilities.document_symbol_provider.is_some(),
        workspace_symbols = capabilities.workspace_symbol_provider.is_some(),
        pull_diagnostics = capabilities.diagnostic_provider.is_some(),
        code_actions = capabilities.code_action_provider.is_some(),
        execute_command = capabilities.execute_command_provider.is_some(),
        "initialize handshake complete; server capabilities captured"
    );

    let app = Arc::new(App::new(Arc::new(client)));
    mcp::serve(app).await.context("MCP server failed")?;
    tracing::info!("MCP transport closed; shutting down");
    Ok(())
}
