# Test projects

Small, self-contained sample projects used to exercise `lsp-mcp` end-to-end
against **real** language servers (the process-level tests under `tests/` use a
fake server instead). Each project is idiomatic for its language, contains
cross-file references and documented symbols so every `lsp-mcp` tool has
something to act on, and carries one deliberately-introduced diagnostic so the
`diagnostics` tool returns something out of the box.

They also double as the fixtures for `tests/language_servers.rs`, which runs
`cargo test --test language_servers` against each project and its real server
when that server (and, for TypeScript/Go, its toolchain) is installed, skipping
the rest.

| Directory     | Language                 | Language server              |
| ------------- | ------------------------ | ---------------------------- |
| `c-cpp/`      | C++ (clangd reads C too) | `clangd`                     |
| `typescript/` | TypeScript + JavaScript  | `typescript-language-server` |
| `rust/`       | Rust                     | `rust-analyzer`              |
| `go/`         | Go                       | `gopls`                      |

## Running a project with `lsp-mcp`

`lsp-mcp` does not tell the language server which project to open — it forwards
your MCP client's requests and lets the server decide. Real servers fall back to
their **process working directory** when the client sends no root, so start
`lsp-mcp` from inside the project you want it to index and pass **absolute**
file paths to the tools.

For example, to drive the Rust project:

```sh
cargo build --release
cd test-projects/rust
/path/to/lsp-mcp/target/release/lsp-mcp rust-analyzer
```

Point your MCP client at that process (see the [root README](../README.md) for
the `mcpServers` shape). The same pattern applies to the others; each project's
own README lists the exact server command and any one-time setup (installing
dependencies, or configuring the language server where needed).

### Server commands

| Project      | `lsp-mcp` command                               |
| ------------ | ----------------------------------------------- |
| `c-cpp`      | `lsp-mcp clangd`                                |
| `typescript` | `lsp-mcp typescript-language-server -- --stdio` |
| `rust`       | `lsp-mcp rust-analyzer`                         |
| `go`         | `lsp-mcp gopls`                                 |

## What to try

Every project is shaped so the read/explore tools find something interesting:

- **`definition` / `references` / `rename`** — each project splits a symbol
  (a `Person`-style struct/interface/type) across two files, so it is defined in
  one and used in the other.
- **`hover`** — every public symbol carries a doc comment.
- **`documentSymbol` / `workspaceSymbol`** — several functions, types, and
  methods across more than one file.
- **`diagnostics`** — each project includes one clearly-commented deliberate
  warning or error.
- **`codeAction` / `applyCodeAction`** — servers offer quick fixes for the
  deliberate diagnostics (for example, "prefix with an underscore" in Rust).
- **`format`** — the sources are hand-formatted; reformatting one (or a range)
  shows the edit.

The deliberate diagnostics are isolated and commented, so removing them leaves a
clean, compiling project. Note that in the **TypeScript** and **Go** projects the
deliberate issue is a type/compile error, so `tsc --noEmit` and `go build` fail
until it is removed — that is intentional, and the language servers still
offer hover, navigation, and quick fixes for the rest of the file.
