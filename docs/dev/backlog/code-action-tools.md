---
type: ChangeRequest
kind: feature
title: Code-action tools
description: List the code actions that apply at a position and apply a chosen one.
state: done
priority: medium
tags: [dev, tools, lsp]
owner: maintainer
verified: { by: maintainer, at: 2026-10-11T15:10:37Z }
---

# Problem

An agent that spots a problem from diagnostics has no way to ask what fixes are
available or to apply one, so it must hand-edit code the server could otherwise
fix correctly.

# Proposal

Add two tools. **List code actions** calls `textDocument/codeAction` for a
position (optionally scoped to the diagnostics there) and returns the available
actions with enough identity to choose among them. **Apply code action** takes a
chosen action, resolves it if the server requires it via `codeAction/resolve`,
and applies the resulting change — either a `WorkspaceEdit`, or a command run
through `workspace/executeCommand`. Applying a `WorkspaceEdit` writes the
affected files.

# Decisions

- **Listing and applying are separate tools** — the agent can inspect the options
  before committing to a write.
- **Actions are resolved before use when the server defers them** — some servers
  return actions whose edits arrive only from `codeAction/resolve`.
- **Applied edits are written to disk by the adapter** — the same
  `WorkspaceEdit` application helper used here is reused by the write tools
  (see [Write tools: rename and format](write-tools-rename-format.md)).
- **The result reports what changed** — which files were written, so the agent
  knows the effect of the call.

# Acceptance criteria

- Given a position with an available fix, listing code actions returns one or
  more actions with identifiable titles.
- Applying a chosen action writes the expected change to disk and reports the
  affected files.
- A code action delivered via `codeAction/resolve` is resolved before being
  applied.
- A code action delivered as a command is executed; one delivered as a
  `WorkspaceEdit` is written.
- Applying an action when none is available, or with an unknown action, returns a
  clear error result.

# Implementation plan

## Approach

This lands as **one coherent deliverable** — two new tools, plus the shared
`WorkspaceEdit` application helper they and the follow-on write tools both use —
so it is **not** decomposed into sub-tasks. It builds on the done
[MCP stdio server and tool surface](mcp-stdio-server.md) and
[Read/explore tools](read-explore-tools.md): the MCP server, the shared tool
infrastructure in `src/tools/mod.rs`, `App`, the LSP client, and the
`DiagnosticsCache` are reused unchanged except where this CR explicitly extends
them.

### Scope boundary

This CR adds exactly **two tools** and, crucially, introduces the **shared
`WorkspaceEdit` application helper** on which the later
[Write tools: rename and format](write-tools-rename-format.md) CR depends. The
helper is a clearly reusable, crate-internal module with a documented API so CR5
reuses it **unchanged**. Rename and format are **not** implemented here: no
`textDocument/rename`, `textDocument/prepareRename`, `textDocument/formatting`,
or `textDocument/rangeFormatting` request is sent, and nothing that only those
features need is added.

### Two tools, one LSP request each

Listing and applying are deliberately separate so the agent can inspect the
options before committing to a write:

| Tool              | LSP request                                                                                       | Kind             |
| ----------------- | ------------------------------------------------------------------------------------------------- | ---------------- |
| `codeAction`      | `textDocument/codeAction`                                                                         | position (read)  |
| `applyCodeAction` | `codeAction/resolve` + `workspace/executeCommand` (as needed) + the shared `WorkspaceEdit` helper | position (write) |

`codeAction` is a **read** tool: it never writes. `applyCodeAction` is the only
tool here that writes files, and the only call routed through the `WorkspaceEdit`
helper.

### How the agent selects an action (decided)

Three identities are possible: **(a)** an index into a freshly re-fetched list,
**(b)** the action's `title`, or **(c)** the whole action object echoed back.

**Decision — (a): the list tool returns a stable `index` per action (alongside its
`title` for readability), and the apply tool re-fetches the list from the same
arguments, picks the action at that `index`, and applies it.**

Why (a):

- It is the only option that lets the apply tool **know what is available**: it
  re-lists, so it can return the acceptance-criterion error "no code actions are
  available at this position" when the list is empty, and "no code action at index
  N" when the index is out of range. Option (c) cannot distinguish these without
  also re-listing.
- It is the **most ergonomic identity** for the agent: a small integer it echoes,
  not a large nested JSON object. The `title` is still returned so the agent can
  reason about and confirm the choice in natural language.
- It makes **resolve integrate naturally**: the re-listed action carries its
  `data` field, which is exactly what `codeAction/resolve` needs, so option (c)'s
  protocol advantage is preserved — we simply obtain the object from the fresh
  list rather than from the agent.
- It sidesteps **duplicate titles**, which break option (b): two actions may share
  a title (e.g. two "Add missing import"), and a first-match rule would apply the
  wrong one.

Rejected: **(b)** because titles are not unique and there is no reliable tie-break;
**(c)** because, without a re-list, it cannot satisfy the "none available / unknown
action" acceptance criterion and it forces the agent to echo and re-serialize a
large object.

Failure modes of (a), and their mitigations:

- **The list can change between the list and apply calls** — the document was
  edited, or the server is nondeterministic — so `index` may select a different
  action. The apply tool bounds-checks the index and returns a clear error when it
  is out of range, and accepts an **optional `title`** on the apply call: when the
  title is supplied and does not match the re-listed action's title, the tool
  returns a clear error instead of applying the wrong action. This is the
  documented, deliberate answer to index drift.
- **The re-list must be reproduced exactly** — the apply tool takes the _same_
  argument shape as the list tool (position, `includeDiagnostics`, `only`), so the
  agent reuses the list call's arguments and appends the chosen `index`/`title`;
  omitting a filtering argument can shift the list, which the title guard surfaces.

### Resolve before apply

Some servers return a `CodeAction` with neither `edit` nor `command`, deferring the
payload to `codeAction/resolve`. After selecting the action from the re-listed
list, the apply flow is:

1. Decode the selected `CodeActionOrCommand`.
2. If it is a `Command` → run it through `workspace/executeCommand` (see below).
3. If it is a `CodeAction` whose `edit` and `command` are **both** absent →
   send `codeAction/resolve` with the action and use the returned action. This is
   the only path to the payload, so it is attempted whenever the action is
   otherwise empty; if the server rejects the request the tool returns a clear
   error naming `codeAction/resolve`.
4. Apply the resolved action:
   - `edit` present → apply the `WorkspaceEdit` through the shared helper;
   - `command` present → execute it through `workspace/executeCommand`;
   - **both** present → apply the edit first, then execute the command, matching
     the protocol's note that a code action's edit is performed before its command;
   - still neither → a clear error ("the action produced no edit or command to
     apply").

`codeAction/resolve` takes the `CodeAction` object itself (there is no wrapper
params type), so no builder is needed — the action is serialized directly.

### Apply outcomes: a `WorkspaceEdit` or a `Command`

- **`WorkspaceEdit`** → written to disk by the shared helper, which returns the
  files changed/created/renamed/deleted for the result.
- **`Command`** → sent as `workspace/executeCommand` with `ExecuteCommandParams
{ command, arguments }`. The command may have side effects the adapter cannot
  observe, so the result reports the command that was executed (and its raw
  response) rather than claiming specific files changed. A `Command` action is
  gated on the server's `executeCommandProvider` capability.

### The shared `WorkspaceEdit` application helper

A new crate-internal module **`src/workspace_edit.rs`** (declared as
`mod workspace_edit;` in `src/main.rs`, a peer of `src/diagnostics.rs`, both being
services the tool layer uses). It is deliberately **MCP-agnostic** — it depends on
`lsp_types`, `crate::error`, and `std::fs`, never on `App`, the MCP server, or a
tool argument type — so CR5's rename and format reuse it verbatim. Its documented
API is:

- `apply(edit: &WorkspaceEdit) -> Result<AppliedEdit, Error>` — the entry point for
  code actions and rename.
- `apply_text_edits(uri: &Uri, edits: &[TextEdit]) -> Result<PathBuf, Error>` — the
  per-file primitive, exposed because `textDocument/formatting` returns a bare
  `TextEdit[]` rather than a `WorkspaceEdit`, so format can call it directly.
- `AppliedEdit { files_changed, files_created, files_renamed, files_deleted }` with
  an `affected()` helper returning the deduped paths touched, for the result text.

Behaviour, decided and documented in the module:

- **Both shapes are handled.** When `document_changes` is present it is preferred
  (per the protocol); otherwise `changes` is used. `document_changes` covers both
  `DocumentChanges::Edits(Vec<TextDocumentEdit>)` and
  `DocumentChanges::Operations(..)`, which mixes `TextDocumentEdit`s with
  `CreateFile`/`RenameFile`/`DeleteFile` resource operations.
- **Edit ordering.** A `WorkspaceEdit`'s edits all address the _initial_ document
  version, so within one file edits are sorted by range **in descending order**
  (by start line, then start character, breaking ties by end) and applied last-to-
  first; this keeps every earlier range valid as later text length changes. Edits
  across different files are independent. An `AnnotatedTextEdit` is unwrapped to
  its inner `TextEdit` (annotations are ignored).
- **Position → offset.** LSP positions are **UTF-16** code units, while Rust
  strings are UTF-8, so the helper converts a `(line, character)` to a byte offset
  with a UTF-16-aware scan (counting code units) rather than by byte or `char`
  index; getting this wrong would corrupt edits on non-ASCII lines, so it gets a
  dedicated unit test.
- **Read/write cycle.** Each affected file is read as UTF-8 (a non-UTF-8 or
  unreadable file is a clear error), its edits are applied in memory, and the whole
  file is written back with `std::fs::write`. Atomic replacement is out of scope;
  the write is reported so a partial multi-file apply is still visible in the result.
- **Resource operations** create (honouring `overwrite`/`ignoreIfExists`, creating
  parent directories), rename (honouring `overwrite`/`ignoreIfExists`), and delete
  (honouring `recursive`/`ignoreIfNotExists`), each as native `std::fs` calls; the
  operations in a `document_changes` array are applied **in order**, because a
  rename can precede edits that target the new path.
- **Errors.** A text edit for a file missing on disk, a URI that is not a file path,
  and a range past the end of a document are all clear `Error` values (R16) that
  the tool maps to an `isError` result — never a panic. Domain failures use
  `Error::Protocol(..)`; I/O failures use `Error::Io` (via `?`).

No server re-synchronisation is done inside the helper: `App::open_document`
already re-reads the file and emits a full-text `didChange` when the text differs,
so the next position-based request reconciles the server automatically.

### Message builders and client capabilities (`src/lsp/message.rs`)

Add three method-name constants beside the existing ones — `CODE_ACTION` =
`textDocument/codeAction`, `CODE_ACTION_RESOLVE` = `codeAction/resolve`, and
`EXECUTE_COMMAND` = `workspace/executeCommand` — and two `lsp-types`-derived
builders:

- `code_action_params(uri, range, diagnostics, only) -> CodeActionParams`, composing
  `TextDocumentIdentifier` + `Range` + `CodeActionContext { diagnostics, only, ..
}`; `only` is `Option<Vec<CodeActionKind>>` and `diagnostics` is the list of
  diagnostics at the position.
- `execute_command_params(command, arguments) -> ExecuteCommandParams` with
  `ExecuteCommandParams { command, arguments, work_done_progress_params }`.

`initialize_params`/`client_capabilities` is extended so a real server enables the
features: advertise `textDocument.codeAction` (with `code_action_literal_support`
so servers return `CodeAction` objects rather than bare `Command`s,
`data_support`, and `resolve_support`), `workspace.workspaceEdit` with
`document_changes: true` and `resource_operations` = create/rename/delete (so
servers may send `documentChanges` and resource operations, which the helper
handles), and `workspace.executeCommand`. All of this is additive to the existing
default `ClientCapabilities`.

### Capability gating (R20)

`ServerCapabilities` exposes `code_action_provider` and `execute_command_provider`.
`App` gains two predicates mirroring `supports_definition()`:

- `supports_code_actions()` → `code_action_provider.is_some()`;
- `supports_execute_command()` → `execute_command_provider.is_some()`.

Both tools check `supports_code_actions()` **first** and return a clear `isError`
result naming `codeActionProvider` when it is absent. Applying a `Command` action
additionally checks `supports_execute_command()` and names `executeCommandProvider`
when it is absent. No request is sent that would fail obscurely.

### Arguments

All argument types derive `serde::Deserialize` + `rmcp::schemars::JsonSchema`
(`#[schemars(crate = "rmcp::schemars")]`) with `#[schemars(description = "…")]` on
every field:

- **`codeAction`** — a `CodeActionArgs` that composes the shared `PositionArgs`
  (`#[serde(flatten)]`) and adds `include_diagnostics: Option<bool>` (default true)
  and `only: Option<Vec<String>>` (code-action kinds to request).
- **`applyCodeAction`** — an `ApplyCodeActionArgs` that composes `CodeActionArgs`
  (`#[serde(flatten)]`) and adds `index: u32` and `title: Option<String>` (the
  drift guard). If the schemars flatten of an already-flattening struct proves
  awkward, the fallback is to list the shared fields directly on the apply struct;
  the tool schemas stay the source of truth either way.

Both position-based tools open the document on demand through `App::open_document`
and validate with the shared `validate_position` (R4), exactly as `definition`
does. The requested `CodeActionParams.range` is the **zero-width range at the
validated position**.

**Including diagnostics.** When `include_diagnostics` is not `false`, the list tool
gathers the diagnostics at the position for `CodeActionContext.diagnostics`, because
many servers only offer a quickfix when handed the diagnostic it resolves. This
reuses the existing retrieval logic: a small `pub(crate)` helper is extracted from
`src/tools/diagnostics.rs` (e.g. `file_diagnostics(app, path) -> Result<Vec<Diagnostic>,
CallToolResult>` — pull when `supports_pull_diagnostics()`, otherwise the push
cache) and both the `diagnostics` tool and the code-action tool use it, so there is
one source of truth. The gathered diagnostics are filtered to those intersecting
the position and included best-effort: if the server reports none, or the file is
unreadable, the list call proceeds with an empty `diagnostics` array rather than
failing.

### Results

Every tool returns `Ok(success_result(text, structured))` and maps expected
failures to `Ok(error_result(..))` (`isError = true`) via the existing helpers,
matching the read tools:

- **`codeAction`** — `{ actions: ActionView[], raw }`. Each `ActionView` carries
  `index` (the identity to apply), `title`, optional `kind`, optional `isPreferred`,
  optional `disabled` (rendered as its reason), `hasEdit`, `hasCommand`, and the
  command id when the action is a bare `Command`. The raw payload is the decoded
  `CodeActionResponse`, included for fidelity. The text block lists one action per
  line as `[index] title (kind[, preferred][, disabled])`. An empty response yields
  a clear "no code actions available at this position" text with `actions: []`.
- **`applyCodeAction`** — `{ title, filesChanged, filesCreated, filesRenamed,
filesDeleted, executedCommand, raw }`. `filesChanged` (and the created/renamed/
  deleted lists) come from the helper's `AppliedEdit`; `executedCommand` names the
  command when a command was run; `raw` carries the resolved action and/or the
  command result. The text block states which files changed (R15) so the agent
  learns the effect of the write.

Errors are data: an out-of-range `index`, a `title` that does not match the
re-listed action, an empty list at apply time, a malformed action, an action that
resolves to neither edit nor command, a missing `codeActionProvider`/
`executeCommandProvider`, an unreadable file, and a failed LSP request all return an
`isError` result with a message an agent can act on — never a panic.

### Testing strategy

Same shape as the done CRs: fast in-process unit tests for the pure pieces, plus a
process-level end-to-end test over real MCP stdio.

- **Unit tests.** `src/lsp/message.rs`: serialized field names for
  `code_action_params` (`textDocument`, `range`, `context.diagnostics`,
  `context.only`) and `execute_command_params` (`command`, `arguments`), and the new
  advertised client capabilities. `src/workspace_edit.rs`: applying `changes`;
  applying `document_changes` as edits and as operations (create/rename/delete);
  the descending-order correctness (two edits on one line/text); the UTF-16 offset
  conversion on a non-ASCII line; unwrapping an `AnnotatedTextEdit`; and a clear
  error for a missing file and an out-of-range edit. `src/tools/code_action.rs` and
  `src/tools/apply_code_action.rs`: decoding a `CodeActionResponse` into views,
  rendering, and the error cases (unknown index, empty list, action with no edit or
  command). `src/app.rs`: the two new capability predicates.
- **End-to-end (`tests/mcp.rs` + `src/bin/fake-lsp-server.rs`).** The fake server
  advertises `codeActionProvider` (with `resolveProvider`), `executeCommandProvider`,
  and the sync capability, and answers `textDocument/codeAction`,
  `codeAction/resolve`, and `workspace/executeCommand`. It returns a deterministic,
  mixed list: one quickfix with a direct `WorkspaceEdit` (`changes`); one action
  with only `data`, resolved by `codeAction/resolve` into a `documentChanges` edit;
  and one bare `Command`. On `workspace/executeCommand` it mutates the target file
  so the test can prove the command actually ran. The test drives the compiled
  binary, writes a **temp file**, and asserts: `tools/list` now lists all **eight**
  tools with descriptions and schemas; `codeAction` returns the three actions with
  indices and titles; applying the direct-edit action reports `filesChanged`
  including the temp path **and the file on disk really changed**; applying the
  resolve-only action resolves before applying (again asserting the on-disk change);
  applying the command action reports `executedCommand` and the file changed by the
  fake server; applying an out-of-range index returns `isError` with a clear
  message; and a `none`-mode server (advertising no `codeActionProvider`) makes both
  tools return `isError` naming `codeActionProvider`. Temp files are cleaned up and
  stdout carries only MCP messages throughout.

### Docs

Only `docs/architecture.md` is updated, by the doc step below. The
`docs/dev/changelog.md` entry is **not** written here: that is the implementation
step's job (`fawi-implement`), not this plan's.

## Steps

- [x] **1. Add the request builders and client capabilities
      (`src/lsp/message.rs`).** Add the `CODE_ACTION`, `CODE_ACTION_RESOLVE`, and
      `EXECUTE_COMMAND` method constants; add `code_action_params(uri, range,
diagnostics, only)` (`CodeActionParams`) and `execute_command_params(command,
arguments)` (`ExecuteCommandParams`) beside the existing builders; extend
      `client_capabilities` to advertise `textDocument.codeAction` (literal, data,
      and resolve support), `workspace.workspaceEdit` (`documentChanges` + create/
      rename/delete resource operations), and `workspace.executeCommand`. Unit-test
      the serialized field names and the new advertised capabilities. _(R11, R12,
      R20; shared by both tools and the helper.)_
- [x] **2. Add capability helpers to `src/app.rs`.** `supports_code_actions()`
      (`code_action_provider`) and `supports_execute_command()`
      (`execute_command_provider`), mirroring `supports_definition`. Unit-test each
      against a synthetic `ServerCapabilities`. _(R20; the clear-error contract.)_
- [x] **3. Build the shared `WorkspaceEdit` helper (`src/workspace_edit.rs`, new;
      `mod workspace_edit;` in `src/main.rs`).** Implement the documented
      `apply(&WorkspaceEdit) -> Result<AppliedEdit, Error>` and
      `apply_text_edits(&Uri, &[TextEdit]) -> Result<PathBuf, Error>` API, handling
      both `changes` and `document_changes` (edits **and** create/rename/delete
      operations), descending-order in-file edits, UTF-16-aware position→offset
      conversion, the read/write cycle, and clear errors for a missing/unreadable
      file or an out-of-range edit. Unit-test every behaviour above with temp files.
      _(The helper CR5 reuses verbatim; R16, R18.)_
- [x] **4. Extract the shared diagnostics retrieval (`src/tools/diagnostics.rs`).**
      Factor the pull/push retrieval into a small `pub(crate)` helper returning the
      file's `Vec<Diagnostic>` (pull when `supports_pull_diagnostics()`, else the
      push cache), and have the `diagnostics` tool use it so there is one source of
      truth. Unit-test the helper's pull and push paths. _(Feeds the code-action
      list's diagnostic scoping; R11, R15.)_
- [x] **5. Implement the `codeAction` list tool (`src/tools/code_action.rs`).** Gate
      on `supports_code_actions`, open the document on demand, validate the position,
      build a zero-width range, gather the intersecting diagnostics (unless
      `include_diagnostics` is false) via the step-4 helper, send
      `textDocument/codeAction` with `only`, decode the `CodeActionResponse`, and
      return an `ActionView` per action (index, title, kind, preferred, disabled,
      hasEdit, hasCommand, command) as a structured result with the raw payload. _(R11,
      R4, R15, R16, R20; AC "lists code actions".)_
- [x] **6. Implement the `applyCodeAction` tool (`src/tools/apply_code_action.rs`).**
      Gate on `supports_code_actions`, re-list from the same arguments, select by
      `index` (bounds-checked, with an optional `title` drift guard), resolve via
      `codeAction/resolve` when the action has neither edit nor command, then apply
      the `WorkspaceEdit` through the step-3 helper and/or execute the `Command` via
      `workspace/executeCommand` (gated on `supports_execute_command`). Return the
      files changed/created/renamed/deleted and the executed command, with the raw
      payload; map every failure to a clear `isError` result. _(R12, R15, R16, R20;
      AC "applies an action", "resolves before applying", "command vs WorkspaceEdit",
      "none available / unknown action".)_
- [x] **7. Register the tools (`src/mcp/server.rs`).** Add the two thin `#[tool]`
      methods (names `codeAction` and `applyCodeAction`, precise descriptions, shared
      `Parameters<..>` argument types) delegating to `tools::*`, and extend
      `get_info`'s instructions to describe the read tools and the two act tools.
      _(R3, R11, R12; AC "list tools and schemas".)_
- [x] **8. Extend the post-handshake capability log (`src/main.rs`).** Log
      `code_actions` and `execute_command` alongside the existing flags. _(R17;
      observability.)_
- [x] **9. Extend the fake LSP server (`src/bin/fake-lsp-server.rs`).** Advertise
      `codeActionProvider` (with `resolveProvider`) and `executeCommandProvider`;
      answer `textDocument/codeAction` with a deterministic mixed list (direct-edit,
      resolve-only, and bare-`Command` actions), `codeAction/resolve` with the
      resolved action, and `workspace/executeCommand` by mutating the target file so
      execution is observable. Omit `codeActionProvider` in the `none` mode so the
      gate is exercised. Stay dependency-light. _(Enables the end-to-end ACs.)_
- [x] **10. Extend the end-to-end MCP tests (`tests/mcp.rs`).** Assert `tools/list`
      lists all **eight** tools; add a test that lists and applies code actions over
      a temp file, asserting the direct-edit write reaches disk, the resolve-only
      action is resolved before applying, the command action is executed (file
      changed by the fake server), an out-of-range index yields an `isError`, and a
      `none`-mode server yields `isError` naming `codeActionProvider`; assert no
      panic and clean stdout/stderr discipline. _(R11, R12, R15, R16, R17, R20; all
      acceptance criteria.)_
- [x] **11. Document the new tools and the helper in `docs/architecture.md`.**
      Extend the module-boundary table with `src/workspace_edit.rs` and the two new
      `tools/` modules; add an act/modify section describing `codeAction` and
      `applyCodeAction`, the re-fetch-and-index selection with resolve, the two apply
      outcomes (edit vs command), and the shared `WorkspaceEdit` helper's contract
      (both edit shapes, descending-order application, UTF-16 offsets, resource
      operations) that CR5 reuses; and extend the data-flow notes to cover writing
      files back to disk. Keep the 1-based convention and stdout discipline notes
      current. _(R15, R17, R18, R20; keeps the boundary doc from drifting.)_
- [x] **12. Final verification.** Run `CARGO_NET_OFFLINE=true cargo build` and
      `CARGO_NET_OFFLINE=true cargo test` from the project root; both must pass with
      no contradictory warnings. Add **no** new dependency — the code-action and
      workspace-edit types are already available via `lsp-types`. The
      `docs/dev/changelog.md` entry is added by the implementation step, not this
      plan. _(All criteria.)_

# Verification

Verified in the current working tree: `CARGO_NET_OFFLINE=true cargo build` and
`CARGO_NET_OFFLINE=true cargo test` both pass (127 unit tests, 3 CLI tests, and 4
end-to-end MCP tests, 0 failures). This repository is not a Git repository, so no
commit is linked.
