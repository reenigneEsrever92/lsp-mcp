# lsp-mcp

A single-binary adapter that exposes an arbitrary [Language Server Protocol][lsp]
(LSP) server to a [Model Context Protocol][mcp] (MCP) client over stdio.

An agent that can only search text is slow and imprecise at understanding code.
By bridging an LSP server to MCP, `lsp-mcp` gives the agent precise language
intelligence — where a symbol is defined, what references it, its type and
documentation, the symbols in a file, the diagnostics a file has, which code
actions apply, and how to rename or format code — so it can explore a codebase
and make changes efficiently.

It is **server-agnostic**: you name an LSP server program on the command line and
`lsp-mcp` forwards that server's own arguments to it. Nothing about any specific
server is hardcoded; behaviour is driven by the capabilities the spawned server
advertises.

## Features

The adapter exposes ten MCP tools:

| Tool              | What it does                                                                                        |
| ----------------- | --------------------------------------------------------------------------------------------------- |
| `definition`      | Locate the definition of the symbol at a position.                                                  |
| `references`      | List every reference to the symbol at a position.                                                   |
| `hover`           | Show the type, signature, and documentation at a position.                                          |
| `documentSymbol`  | List the symbols in a file.                                                                         |
| `workspaceSymbol` | Search the workspace for symbols matching a query.                                                  |
| `diagnostics`     | Report a file's diagnostics (pull `textDocument/diagnostic`, else the push cache).                  |
| `codeAction`      | List the code actions available at a position. Read-only.                                           |
| `applyCodeAction` | Apply a chosen code action, writing its edits to disk and/or running its command. **Writes files.** |
| `rename`          | Rename the symbol at a position everywhere it is used. **Writes files.**                            |
| `format`          | Format a whole file or a range. **Writes files.**                                                   |

Position-based tools take a file path plus a **1-based** line and column (what you
see in an editor). Results are structured and agent-friendly — document URI,
position, surrounding code context — with the raw LSP payload included where it
adds fidelity. A server that does not advertise a capability yields a clear
error naming the missing feature rather than an obscure failure.

## Requirements

- A [Rust toolchain][rust] (to build from source).
- The LSP server you want to bridge must be installed and on your `PATH` (for
  example `rust-analyzer`, `typescript-language-server`, `pyright-langserver`,
  `gopls`, `clangd`).

## Install

Build from source:

```sh
cargo build --release
```

The binary lands at `target/release/lsp-mcp`. To install it onto your `PATH`:

```sh
cargo install --path .
```

## Usage

```text
lsp-mcp <server-command> [-- <arg>...]
```

`lsp-mcp` spawns `<server-command>`, forwards every argument after `--` to it, and
then speaks MCP over its own stdio. For example, to bridge `rust-analyzer`:

```sh
lsp-mcp rust-analyzer
```

Servers that take their own flags receive them after `--`:

```sh
lsp-mcp typescript-language-server -- --stdio
```

### Configure your MCP client

Point your MCP client at the `lsp-mcp` binary, naming the LSP server as an
argument. For example, in a typical `mcpServers` configuration:

```json
{
  "mcpServers": {
    "rust": {
      "command": "/absolute/path/to/lsp-mcp",
      "args": ["rust-analyzer"]
    },
    "typescript": {
      "command": "/absolute/path/to/lsp-mcp",
      "args": ["typescript-language-server", "--", "--stdio"]
    }
  }
}
```

Run one `lsp-mcp` process per language server. The adapter is the bridge; it does
not itself decide which files or projects are relevant — the server does.

## How it works

`lsp-mcp` launches the LSP server, speaks the LSP client protocol to it over the
child process's stdio (`Content-Length` framing, the `initialize` handshake,
per-request timeouts, and on-demand document sync), and exposes that server to an
MCP client over its own stdio as the tools above.

```mermaid
flowchart LR
    McpClient[MCP client] -->|newline-delimited JSON-RPC| Adapter[lsp-mcp]
    Adapter -->|Content-Length LSP frames| LspServer[LSP server]
```

stdout carries only the MCP transport; every log and diagnostic goes to stderr, so
the two channels never mix.

## Documentation

The full documentation lives under [`docs/`](docs/index.md):

- [Overview](docs/overview.md) — what `lsp-mcp` is, who it serves, and what it
  deliberately leaves out.
- [Requirements](docs/requirements.md) — the users, use cases, technology
  choices, and numbered requirements.
- [Architecture](docs/architecture.md) — the module boundaries and data flow.
- [Development](docs/dev/index.md) — the change-driven workflow, the backlog of
  change requests, and the changelog.

## Development

```sh
cargo build      # build
cargo test       # run the unit and process-level integration tests
cargo fmt        # format
cargo clippy     # lint
```

The integration tests spawn a test-only `fake-lsp-server` binary
(`src/bin/fake-lsp-server.rs`) in place of a real language server, so the whole
adapter can be exercised without installing one.

## License

Licensed under the MIT License.

[lsp]: https://microsoft.github.io/language-server-protocol/
[mcp]: https://modelcontextprotocol.io/
[rust]: https://rustup.rs/
