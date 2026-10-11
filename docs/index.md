# lsp-mcp

`lsp-mcp` is a single-binary adapter that exposes an arbitrary Language Server
Protocol (LSP) server to a Model Context Protocol (MCP) client over stdio. It
lets an agent explore a codebase and make changes using real language
intelligence — definitions, references, hover, symbols, diagnostics, code
actions, rename, and formatting — instead of text search.

## Areas

- [Overview](overview.md) — what `lsp-mcp` is, who it serves, and what it
  deliberately leaves out.
- [Requirements](requirements.md) — the users, use cases, technology choices,
  and numbered requirements behind the project.
- [Architecture](architecture.md) — the module boundaries and data flow of the
  adapter.
- [Front matter schema](frontmatter.md) — the metadata fields used across this
  bundle.
- [Development](dev/index.md) — the change-driven workflow, the backlog of
  change requests, and the changelog.
