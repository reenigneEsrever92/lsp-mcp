//! The MCP server layer.
//!
//! Nothing here knows about LSP framing: the server holds the shared
//! [`App`](crate::app::App) and delegates each tool to [`crate::tools`], which
//! talks to the LSP client. [`serve`] drives the MCP transport over stdio.
//!
//! - [`server`] — [`LspServer`](server::LspServer): the `#[tool_router]` /
//!   `#[tool_handler]` server and the stdio serve entry point.

pub mod server;

pub use server::serve;
