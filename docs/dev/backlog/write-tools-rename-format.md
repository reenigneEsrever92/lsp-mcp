---
type: ChangeRequest
kind: feature
title: Write tools: rename and format
description: Rename the symbol at a position and format a file or range.
state: done
priority: medium
tags: [dev, tools, lsp]
owner: maintainer
verified: { by: maintainer, at: 2026-10-11T15:25:10Z }
---

# Problem

Beyond reading, the agent needs the server to make two common safe edits it does
well — renaming a symbol everywhere it is used, and formatting code — but no tool
offers them.

# Proposal

Add a **rename** tool and a **format** tool. Rename calls
`textDocument/prepareRename` to validate the position and obtain the current
name, then `textDocument/rename` with the new name, applying the returned
`WorkspaceEdit` across all affected files. Format calls `textDocument/formatting`
for a whole file or `textDocument/rangeFormatting` for a range, and applies the
returned edits. Both tools write files and report which files changed.

# Decisions

- **Rename validates before editing** — `prepareRename` confirms the position
  names a renameable symbol and yields the old name, so failures are clean.
- **Edits are applied through the shared `WorkspaceEdit` helper** — introduced by
  the code-action tools (see [Code-action tools](code-action-tools.md)), so file
  writing behaves identically across write-capable tools.
- **The result lists the files changed** — the agent learns the scope of the edit.
- **Formatting is whole-file or range** — chosen to cover both common cases.

# Acceptance criteria

- Renaming a symbol updates every reference across all affected files and reports
  those files.
- Renaming at a position that is not a renameable symbol returns a clear error
  result without writing anything.
- Formatting a file writes formatted content and reports the file; formatting a
  range changes only that range.
- If a server does not support rename or formatting, the tool reports that
  clearly.

# Implementation plan

## Approach

One coherent deliverable: two write tools, `rename` and `format`, both writing
files through the shared [`src/workspace_edit.rs`](../../architecture.md) helper
**reused unchanged**. No new dependency; the helper, the `App`, and the tool-layer
conventions are all reused.

| Tool     | LSP request(s)                                             | Kind             | Arguments                             |
| -------- | ---------------------------------------------------------- | ---------------- | ------------------------------------- |
| `rename` | `textDocument/prepareRename` then `textDocument/rename`    | position (write) | file + 1-based line/column + new_name |
| `format` | `textDocument/formatting` / `textDocument/rangeFormatting` | file or range    | file + optional 1-based range         |

### Rename: prepare, then rename, then apply

1. Gate on `renameProvider` (`App::supports_rename`); a server that does not
   advertise it gets a clear `isError` naming `renameProvider`.
2. Open the document on demand (`App::open_document`) and validate the shared
   1-based position, as the other position tools do.
3. Send `textDocument/prepareRename`. `lsp-types` 0.97 declares
   `PrepareRenameRequest::Params = TextDocumentPositionParams`, so there is **no**
   distinct `PrepareRenameParams` type and the existing
   `text_document_position_params` builder is reused (a thin `prepare_rename_params`
   alias may wrap it for symmetry, but no new payload type is added).
4. Decode the `Option<PrepareRenameResponse>` (three variants plus `null`):
   - `null` → `error_result("the position does not name a renameable symbol")`;
     **nothing is written**.
   - `Range(r)` → renameable; the old name is the document text over `r`.
   - `RangeWithPlaceholder { range, placeholder }` → renameable; old name is the
     `placeholder`.
   - `DefaultBehavior { .. }` → renameable (the server applies its default range);
     old name is unknown and omitted from the result.
5. Send `textDocument/rename` with `RenameParams { text_document_position, new_name }`
   (new `rename_params(uri, position, new_name)` builder).
6. The result is `Option<WorkspaceEdit>`; `null`/absent → a clear `isError`
   ("the server returned no edits"), **nothing written**. Otherwise apply it
   through `workspace_edit::apply` (unchanged).
7. Best-effort refresh of the changed/created paths (see _Editing state_).
8. Structured result:
   `{ oldName?, newName, filesChanged, filesCreated, filesRenamed, filesDeleted, raw }`
   plus a text block naming the files.

Per the request's decision the tool **always** calls `prepareRename` first; a
server that supports rename but not prepareRename returns a clear error naming
`textDocument/prepareRename` rather than an unvalidated write. Falling back to an
unvalidated rename is rejected — it contradicts "validates before editing".

### Format: whole-file or range, one bare `TextEdit[]`

1. Choose the mode from the arguments: an **absent** `range` → whole file via
   `textDocument/formatting`, gated on `documentFormattingProvider`; a **present**
   `range` → `textDocument/rangeFormatting`, gated on
   `documentRangeFormattingProvider`.
2. Open the document on demand; for a range, validate both 1-based ends against
   the text.
3. Build the params — `DocumentFormattingParams { text_document, options }` or
   `DocumentRangeFormattingParams { text_document, range, options }` — from new
   `document_formatting_params`/`document_range_formatting_params` builders and a
   `formatting_options()` helper (tabSize 4, insertSpaces true; fixed in this CR).
4. The response is a bare `Option<Vec<TextEdit>>`. `null`/empty → a success result
   ("already formatted; no changes"), **nothing written**. Otherwise apply through
   `workspace_edit::apply_text_edits(&uri, &edits)` — the bare array is exactly
   what that helper takes.
5. Best-effort refresh of the file.
6. Structured result: `{ file, range?, appliedEdits, filesChanged, raw }` plus a
   text block.

**Argument shape (decision):** `format` takes `file` plus an optional nested
`range` object of 1-based `line`/`column`/`end_line`/`end_column`. The presence of
`range` selects range formatting; its absence means whole-file. A nested object
keeps the schema self-describing and the validation in one place.

### Capability gating (R20)

Add three predicates to `App` reading `rename_provider`,
`document_formatting_provider`, and `document_range_formatting_provider`
(`ServerCapabilities` fields, each `Option<OneOf<bool, _>>`). Each tool checks its
capability **first** and returns a clear `isError` naming the missing capability,
mirroring the existing read/act tools.

### Editing state (decision)

After a write the adapter's in-memory `Documents` store still holds the pre-edit
text and version, so a later position-based request would be validated against
stale text and the server would hold the wrong view. **Decision: refresh the
affected documents after every write.** Each changed/created path is re-read from
disk and passed through `Documents::ensure_open`, which emits `didChange` when the
text differs. This is a new best-effort `App::refresh_documents(&[PathBuf])` that
reuses `App::open_document`; a refresh failure is logged, not fatal (the write
already succeeded), and paths that no longer exist (a rename's old path, a delete)
are skipped. No new `Documents` API is needed.

### Results and errors

Both tools reuse the shared `success_result`/`error_result`/`error_from` helpers:
a human-readable text block plus `structured_content` (the files changed and the
raw LSP payload where useful). Every expected failure — a missing capability, a
non-renameable position, an unreadable file, a bad range, a null edit, an LSP
error response — is returned as an `isError` tool result, never a panic.

## Steps

- [x] Add the method constants `PREPARE_RENAME` (`textDocument/prepareRename`),
      `RENAME` (`textDocument/rename`), `FORMATTING` (`textDocument/formatting`),
      and `RANGE_FORMATTING` (`textDocument/rangeFormatting`) to the `methods`
      module in `src/lsp/message.rs`, plus the params builders `rename_params`,
      `document_formatting_params`, `document_range_formatting_params`, and a
      `formatting_options()` helper. Reuse `text_document_position_params` for
      prepareRename (no distinct `PrepareRenameParams` exists in lsp-types 0.97).
      Extend `client_capabilities()` to advertise `textDocument.rename` (with
      `prepare_support`), `textDocument.formatting`, and
      `textDocument.range_formatting`. Add unit tests for the builders and the
      advertised capabilities.
- [x] Add `App::supports_rename`, `App::supports_document_formatting`, and
      `App::supports_document_range_formatting` to `src/app.rs`, reading
      `rename_provider` / `document_formatting_provider` /
      `document_range_formatting_provider`; extend the `app.rs` capability test.
- [x] Add a best-effort `App::refresh_documents(&[PathBuf])` to `src/app.rs` that
      re-opens each path through `open_document` so `Documents` matches disk after
      a write, logging failures and skipping paths that no longer exist.
- [x] Add to `src/tools/mod.rs`: `pub mod rename; pub mod format;`, the shared
      `RenameArgs { #[serde(flatten)] position: PositionArgs, new_name: String }`,
      `FormatArgs { file: String, range: Option<FormatRange> }`, and
      `FormatRange { line, column, end_line, end_column }` (all 1-based), plus a
      `validate_range` helper that delegates to `validate_position` for both ends
      and rejects an end before the start. Add unit tests for the flat rename
      schema and the range validation.
- [x] Implement `src/tools/rename.rs`: gate on `supports_rename`, open the
      document, validate the position, send `prepareRename`, handle `null` (clear
      `isError`, nothing written) and the three non-null variants (deriving the old
      name from the placeholder or the range text), send `rename` with the new
      name, apply the `WorkspaceEdit` through `workspace_edit::apply`, refresh the
      affected files, and return the structured result listing the files changed.
      This covers the first two acceptance criteria for rename.
- [x] Implement `src/tools/format.rs`: select whole-file vs range from the
      optional `range`, gate on the matching capability, open the document,
      validate a range when present, send `formatting`/`rangeFormatting`, treat
      `null`/empty edits as "no changes", otherwise apply through
      `workspace_edit::apply_text_edits`, refresh the file, and return the
      structured result. This covers both format acceptance criteria.
- [x] Register the two tools in `src/mcp/server.rs`: a `rename` `#[tool]` over
      `RenameArgs` and a `format` `#[tool]` over `FormatArgs`, each delegating to
      `crate::tools::rename::run` / `crate::tools::format::run`, and extend
      `get_info`'s instructions to name them.
- [x] Extend `src/bin/fake-lsp-server.rs`: advertise `renameProvider` (with
      `prepareProvider`), `documentFormattingProvider`, and
      `documentRangeFormattingProvider` in the default capabilities; answer
      `textDocument/prepareRename` (a placeholder range for a renameable position,
      `null` for a non-renameable one), `textDocument/rename` (a multi-file
      `WorkspaceEdit`, deriving a deterministic sibling URI from the last-seen URI
      so the second file is addressable), `textDocument/formatting` (a
      whole-document `TextEdit` computed from the file on disk), and
      `textDocument/rangeFormatting` (a `TextEdit` limited to the requested
      range). In `none` mode advertise none of them so the gates trip.
- [x] Extend `tests/mcp.rs`: update the `tools/list` assertions to the ten tools,
      including the `rename` (`file`/`line`/`column`/`new_name`) and `format`
      (`file`/`range`) schemas; add an end-to-end rename test over **two** temp
      files asserting both are written and reported, and that a non-renameable
      position returns `isError` and leaves the file unchanged; add a format test
      asserting whole-file formatting writes the formatted content and reports the
      file, and that range formatting changes only the range; extend
      `reports_an_unsupported_capability_clearly` with `rename` and `format`
      (whole-file and range) cases naming `renameProvider`,
      `documentFormattingProvider`, and `documentRangeFormattingProvider`.
- [x] Update `docs/architecture.md`: add `src/tools/rename.rs` and
      `src/tools/format.rs` rows to the module table; extend the act/modify tools
      table with `rename` and `format`; document the rename data flow
      (`prepareRename` → `rename` → shared helper → refresh) and the format data
      flow (whole-file/range → `apply_text_edits` → refresh); note the capability
      gating (R20), the stale-`Documents` refresh decision, and that the
      `WorkspaceEdit` helper is reused unchanged.
- [x] Set this request's `state` to `planned` and update the row in
      `docs/dev/backlog/index.md`. (`docs/dev/changelog.md` and `docs/log.md` are
      updated at implementation time, not in this plan.)
- [x] Verify offline: `CARGO_NET_OFFLINE=true cargo build` and
      `CARGO_NET_OFFLINE=true cargo test` from the project root pass, with no new
      dependency added.

# Verification

Verified in this working tree: `CARGO_NET_OFFLINE=true cargo build` and
`CARGO_NET_OFFLINE=true cargo test` both pass (148 unit, 3 CLI, and 6 MCP tests),
with no new dependency added. This is not a git repository, so there is no commit
to link.
