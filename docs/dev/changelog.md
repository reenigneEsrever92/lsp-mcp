---
type: Changelog
title: Changelog
description: Shipped changes, newest first.
tags: [changelog, dev]
status: active
---

# Changelog

Shipped changes are listed newest first under dated headings.

## 2026-10-13

- **Advertise the working directory as the workspace root** — the adapter now
  sends its process working directory in the LSP `initialize` handshake as
  `workspaceFolders` and the legacy `rootUri` (and advertises the
  `workspace.workspaceFolders` client capability), so a server that indexes a
  project on start-up — including one that does not fall back to its own working
  directory, such as `java-lsp` — finds it. Adds a Java sample project and a
  `java-lsp` real-server test. See
  [Advertise the working directory as the workspace root](backlog/workspace-root-from-cwd.md).

## 2026-10-11

- **Merge pushed diagnostics with pulled ones** — the `diagnostics` tool now returns the de-duplicated union of the pull report (`textDocument/diagnostic`) and the push cache (`textDocument/publishDiagnostics`), reporting `mechanism` as `"pull"`, `"publish"`, or `"pull+publish"` from the channels that contributed. See
  [Merge pushed diagnostics with pulled ones](backlog/merge-pushed-diagnostics.md).
- **Write tools: rename and format** — the adapter now registers `rename` (rename the symbol at a 1-based position everywhere it is used via `textDocument/prepareRename` then `textDocument/rename`) and `format` (format a whole file via `textDocument/formatting` or a 1-based range via `textDocument/rangeFormatting`), both applying their edits to disk and reporting the files changed. See
  [Write tools: rename and format](backlog/write-tools-rename-format.md).
- **Code-action tools** — the adapter now registers `codeAction` (list the actions at a position via `textDocument/codeAction`) and `applyCodeAction` (resolve a chosen action via `codeAction/resolve` and apply its `WorkspaceEdit` to disk, or run its command via `workspace/executeCommand`). See
  [Code-action tools](backlog/code-action-tools.md).
- **Read/explore tools** — the adapter now registers `references`, `hover`,
  `documentSymbol`, `workspaceSymbol`, and `diagnostics` alongside `definition`,
  each gated on its advertised capability and returning a structured result with
  URI, 1-based position, code context, and the raw LSP payload, with diagnostics
  served by pull (`textDocument/diagnostic`) or pushed
  (`textDocument/publishDiagnostics`). See
  [Read/explore tools](backlog/read-explore-tools.md).
- **MCP stdio server and tool surface** — the adapter now serves MCP over
  newline-delimited stdio and registers a `definition` tool that returns each
  location's URI, 1-based line and column, surrounding code context, and the raw
  LSP payload as a structured result. See
  [MCP stdio server and tool surface](backlog/mcp-stdio-server.md).
- **Adapter core, CLI, and LSP client transport** — the `lsp-mcp` binary parses
  `lsp-mcp -- <server> [args...]`, spawns the server, and completes the LSP
  `initialize`/`initialized` handshake over hand-rolled `Content-Length`
  framing, with per-request timeouts, exit detection, on-demand document
  sync, and stderr-only logging. See
  [Adapter core, CLI, and LSP client transport](backlog/adapter-core-lsp-transport.md).
