# Directory update log

This log records how the `docs/` bundle itself evolves. Newest entries are
prepended under their own `## YYYY-MM-DD` heading.

## 2026-10-13

- **Backlog**: Added
  [Advertise the working directory as the workspace root](dev/backlog/workspace-root-from-cwd.md),
  implemented it, and recorded it in the [changelog](dev/changelog.md).
- **Architecture**: Noted in [`architecture.md`](architecture.md) that the
  `initialize` handshake advertises the process working directory as the
  workspace root (`workspaceFolders` and the legacy `rootUri`), and added
  `java-lsp` to the real-server test surface.

## 2026-10-11

- **Architecture**: Documented the real-server test surface in
  [`architecture.md`](architecture.md) — the `test-projects/` sample projects
  (C/C++, TypeScript/JavaScript, Rust, Go) and the `tests/language_servers.rs`
  integration test that drives the adapter against their real language servers
  when installed, plus the shared `tests/common/mod.rs` `McpClient` harness.
- **Architecture**: Updated [`architecture.md`](architecture.md) to describe the
  `diagnostics` tool's merged pull ∪ push behaviour and its `mechanism`
  (`"pull"`/`"publish"`/`"pull+publish"`) values, and added a pointer from the
  historical decision in
  [Read/explore tools](dev/backlog/read-explore-tools.md) to
  [Merge pushed diagnostics with pulled ones](dev/backlog/merge-pushed-diagnostics.md).
- **Backlog**: Added a proposed improvement,
  [Merge pushed diagnostics with pulled ones](dev/backlog/merge-pushed-diagnostics.md),
  to merge pull and push diagnostics in the `diagnostics` tool so check/clippy
  lints are not dropped.
- **Reconciliation**: Reconciled the concept documents with the implemented
  adapter — fixed a dangling `requirements.md` link in
  [`architecture.md`](architecture.md), documented the `verified` change-request
  field in [`frontmatter.md`](frontmatter.md), stated that the whole first
  release (R1–R20) has shipped in [`requirements.md`](requirements.md), noted the
  test-only fake-server binary, and advanced the `status` of the overview,
  requirements, architecture, and front-matter pages from `draft` to `active`.
- **Architecture**: Extended [`architecture.md`](architecture.md) with the
  `rename`/`format` write tools, the `textDocument/prepareRename` → rename →
  shared-helper → refresh flow and the whole-file/range format flow, their
  capability gating (R20), the `App::refresh_documents` stale-`Documents` refresh
  decision, and the new message builders and `App` predicates.
- **Architecture**: Extended [`architecture.md`](architecture.md) with the
  `codeAction`/`applyCodeAction` tools, the shared
  [`src/workspace_edit.rs`](architecture.md) `WorkspaceEdit` application helper,
  code-action capability gating, and the act/modify data flow.
- **Architecture**: Extended [`architecture.md`](architecture.md) with
  [`src/diagnostics.rs`](architecture.md) (the push-diagnostics cache), the five
  new `tools/` modules, a read/explore tools table, the `App` capability gating
  that implements R20, and the diagnostics data flow over the notification
  broadcast.
- **Architecture**: Extended [`architecture.md`](architecture.md) with the new
  `mcp/` and `tools/` modules, the shared `App` state, the end-to-end MCP data
  flow, the newline-delimited stdio contract, and the 1-based line/column
  convention the shared tool arguments enforce.
- **Architecture**: Added [`architecture.md`](architecture.md) describing the
  module boundaries, data flow, and stderr/stdout contract of the adapter, and
  linked it from [the bundle index](index.md) and the
  [development index](dev/index.md).
- **Creation**: Initialized the project as an OKF bundle — discovered the
  project kind, users, use cases, and technologies; recorded the requirement
  analysis in [`requirements.md`](requirements.md); and seeded the backlog with
  the initial change requests.
