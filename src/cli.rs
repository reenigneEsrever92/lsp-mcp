//! Command-line interface.
//!
//! The adapter is server-agnostic: the LSP server program is named as a plain
//! positional argument, and only that server's own arguments follow the `--`
//! separator. Everything after the separator is forwarded to the server
//! verbatim, in order:
//!
//! ```text
//! lsp-mcp <server-command> [-- <arg>...]
//! ```

use clap::Parser;

/// Launch an LSP server and expose it to an MCP client over stdio.
#[derive(Debug, Parser)]
#[command(
    name = "lsp-mcp",
    about = "Bridge a Language Server Protocol server to a Model Context Protocol client.",
    version
)]
pub struct Cli {
    /// The LSP server program to launch.
    #[arg(value_name = "SERVER")]
    pub server: String,

    /// The arguments forwarded to the server, after `--`.
    #[arg(last = true, value_name = "ARGS")]
    pub args: Vec<String>,
}

impl Cli {
    /// The server program to launch.
    pub fn command(&self) -> &str {
        &self.server
    }

    /// The arguments forwarded to the server, in order.
    pub fn args(&self) -> &[String] {
        &self.args
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_server_without_arguments() {
        let cli = Cli::try_parse_from(["lsp-mcp", "/usr/bin/lsp-server"]).unwrap();
        assert_eq!(cli.command(), "/usr/bin/lsp-server");
        assert!(cli.args().is_empty());
    }

    #[test]
    fn preserves_argument_order_and_hyphenated_flags() {
        let cli = Cli::try_parse_from([
            "lsp-mcp",
            "rust-analyzer",
            "--",
            "--stdio",
            "--log-file",
            "ra.log",
        ])
        .unwrap();
        assert_eq!(cli.command(), "rust-analyzer");
        assert_eq!(cli.args(), &["--stdio", "--log-file", "ra.log"]);
    }

    #[test]
    fn error_when_no_server_is_given() {
        let result = Cli::try_parse_from(["lsp-mcp"]);
        assert!(result.is_err(), "expected an error when no server is given");
    }
}
