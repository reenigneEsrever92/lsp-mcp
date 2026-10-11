# Directory update log

This log records how the `docs/` bundle itself evolves. Newest entries are
prepended under their own `## YYYY-MM-DD` heading.

## 2026-10-11

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
