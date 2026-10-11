---
type: ChangeRequest
kind: feature
title: Read/explore tools
description: Expose definition, references, hover, document and workspace symbols, and diagnostics.
state: done
verified: { by: maintainer, at: 2026-10-11T14:42:25Z }
priority: high
tags: [dev, tools, lsp]
owner: maintainer
---

# Problem

The value of the adapter is letting an agent understand a codebase, but there are
no tools that answer questions about code: where a symbol is defined, what
references it, its type, the symbols in a file or workspace, or what diagnostics a
file has.

# Proposal

Implement the read/explore tools, each mapping to an LSP request and returning a
structured result:

- **Definition** — `textDocument/definition` for the symbol at a position.
- **References** — `textDocument/references`.
- **Hover** — `textDocument/hover`, yielding type, signature, and documentation.
- **Document symbols** — `textDocument/documentSymbol`.
- **Workspace symbols** — `workspace/symbol`.
- **Diagnostics** — `textDocument/diagnostic` and/or diagnostics received via
  `textDocument/publishDiagnostics`.

Position-based tools take a file and a position, open and update the document on
demand before querying, and return locations with surrounding code context.

# Decisions

- **One tool per LSP capability** — keeps each tool's contract small and its
  description precise, so an agent knows when to call it.
- **Positions are given as file path plus line and column** — the tool converts
  to an LSP `Position` and `Url`, sparing the agent from LSP addressing.
- **Documents are opened on demand** — the tool layer drives the document
  lifecycle from the core transport rather than requiring the agent to open
  files first.
- **Results include code context, not just coordinates** — an agent should not
  have to make a second call to learn what is at a location.
- **Behavior follows the server's advertised capabilities** — if a server does
  not support a capability, the tool reports that clearly instead of failing
  obscurely.

# Acceptance criteria

- Given a real LSP server and a workspace, definition returns the symbol's
  location; references lists its references; hover returns its type or
  documentation; document symbols lists a file's symbols; workspace symbols
  matches a query; diagnostics returns a file's diagnostics.
- Each result includes the file URI, a line and column, and surrounding code
  context.
- A position-based request against a not-yet-open document opens it and succeeds.
- An out-of-range position and a request for a missing file each return a clear
  error result.
- A capability the server does not support is reported clearly.

# Implementation plan

## Approach

This lands as **one coherent deliverable** — five more read tools and a diagnostics
cache, all reusing the existing MCP server, shared tool infrastructure, and LSP
client — so it is **not** decomposed into sub-tasks.

### Scope boundary

The MCP stdio server, the reusable tool infrastructure in `src/tools/mod.rs`, and
the pilot `definition` tool were delivered by the already-done
[MCP stdio server and tool surface](mcp-stdio-server.md). This CR implements only
the **remaining five** read/explore tools — `references`, `hover`, `documentSymbol`,
`workspaceSymbol`, `diagnostics` — against that infrastructure, and makes the six
read tools one coherent surface. The server plumbing is not rebuilt; `definition`
is touched only if the shared helpers change shape under it.

### One tool per LSP request

Each tool maps to a single LSP request and is a thin `#[tool]` method on
`LspServer` that delegates to a function in `src/tools/`:

| Tool              | LSP request                                                                 | Kind     |
| ----------------- | --------------------------------------------------------------------------- | -------- |
| `definition`      | `textDocument/definition` (existing)                                        | position |
| `references`      | `textDocument/references`                                                   | position |
| `hover`           | `textDocument/hover`                                                        | position |
| `documentSymbol`  | `textDocument/documentSymbol`                                               | file     |
| `workspaceSymbol` | `workspace/symbol`                                                          | query    |
| `diagnostics`     | `textDocument/diagnostic` (pull) / `textDocument/publishDiagnostics` (push) | file     |

### Module layout

- `src/lsp/message.rs` — add the method-name constants for the four new requests
  plus the `publishDiagnostics` notification, and thin `lsp-types`-derived builders
  beside the existing `text_document_position_params`:
  `reference_params(uri, position, include_declaration)` (`ReferenceParams`),
  `document_symbol_params(uri)` (`DocumentSymbolParams`),
  `workspace_symbol_params(query)` (`WorkspaceSymbolParams`), and
  `document_diagnostic_params(uri)` (`DocumentDiagnosticParams`). Hover reuses
  `text_document_position_params` (`textDocument/hover` accepts the plain
  `TextDocumentPositionParams` JSON, exactly as `definition` does).
- `src/tools/mod.rs` — register the five new submodules and add the shared
  argument/result pieces they need (see **Arguments** and **Results**): the
  file-only, query-only, and references argument types, and a shared `SymbolView`
  for the two symbol-listing tools.
- `src/tools/references.rs`, `hover.rs`, `document_symbol.rs`, `workspace_symbol.rs`,
  `diagnostics.rs` — one module per tool, each mirroring `definition.rs`: a thin
  `run(app, args) -> Result<CallToolResult, ErrorData>` wrapper and an `_impl` body
  that returns the tool-level error result on expected failure.
- `src/mcp/server.rs` — five new `#[tool]` methods in the `#[tool_router]` impl,
  each delegating to a `tools::*` function; extend the `get_info` instructions to
  describe all six read tools.
- `src/app.rs` — capability helpers for the new features and ownership of the
  diagnostics cache.
- `src/diagnostics.rs` (new) — `DiagnosticsCache`: the consumer that captures
  pushed `publishDiagnostics` notifications (see **Diagnostics**).
- `src/main.rs` — extend the post-handshake capability log to cover the new
  features (it already logs `definition` and `hover`).
- `src/bin/fake-lsp-server.rs`, `tests/mcp.rs` — extended for end-to-end coverage.

### Capability gating (R20)

`ServerCapabilities` exposes `references_provider`, `hover_provider`,
`document_symbol_provider`, `workspace_symbol_provider`, and `diagnostic_provider`.
`App` gains one predicate per feature — `supports_references()`,
`supports_hover()`, `supports_document_symbols()`,
`supports_workspace_symbols()`, and `supports_pull_diagnostics()` — each mirroring
`supports_definition()`: `capabilities().map(|c| c.<field>.is_some()).unwrap_or(false)`.
Every tool checks its predicate **first** and, when it is false, returns a clear
`isError` tool result naming the missing capability (e.g. "the LSP server does
not advertise references support (`referencesProvider`)") rather than sending a
request that fails obscurely. This is the R20 contract.

### Diagnostics — pull and push

The diagnostics tool is the subtle one because LSP offers two mechanisms:

- **Pull** (`textDocument/diagnostic`, LSP 3.17) — gated by the server's
  `diagnosticProvider` capability. The tool sends one request and gets a report
  back synchronously; there is no timing problem.
- **Push** (`textDocument/publishDiagnostics`) — a server-to-client notification
  with **no capability gate** (any server may publish at any time). The client
  already forwards every server notification to a broadcast channel
  (`Client::subscribe`); the tool must consume that stream to see pushed
  diagnostics.

**Decision — prefer pull, fall back to push.** The tool chooses by capability:
when `supports_pull_diagnostics()` is true it uses `textDocument/diagnostic`;
otherwise it reads the push cache. Both are file-scoped, matching this CR's
"file-only" diagnostics argument (workspace-wide `workspace/diagnostic` is out of
scope). No capability is reported as "missing" for push, because push is always
allowed; instead, when no pushed report has arrived the tool says so clearly.

**Capturing pushed diagnostics.** A new `src/diagnostics.rs` holds
`DiagnosticsCache`, a small background consumer:

- `DiagnosticsCache::spawn(receiver: broadcast::Receiver<Notification>)` takes the
  receiver from `client.subscribe()` and `tokio::spawn`s a task that loops on
  `recv()`, keeps the notifications whose `method` is
  `textDocument/publishDiagnostics`, decodes each into `PublishDiagnosticsParams`,
  and stores the latest `Vec<Diagnostic>` per URI keyed by `uri.as_str()`.
- The task must subscribe **before** any tool can trigger a `didOpen`, which is why
  the cache is created in `App::new` (right after the handshake, before MCP
  serving), not lazily on first use. `App` owns an `Arc<DiagnosticsCache>` and
  exposes `diagnostics()`; the consumer task ends when the client (and its
  broadcast sender) is dropped.
- **Timing — decided:** each stored entry carries a monotonic revision stamp. The
  tool reads the URI's current entry/revision, opens the document (so a server that
  publishes on `didOpen` emits a fresh report), then awaits an entry for that URI
  with a **revision newer than the baseline**, under a short bounded timeout
  (`DiagnosticsCache::await_newer(uri, baseline, timeout)`, implemented with a
  `tokio::sync::Mutex<HashMap<...>>` plus a `tokio::sync::Notify` woken on every
  publish). A report pushed _before_ the call is already cached, so it is returned
  immediately; a report pushed _in response to_ the `didOpen` is awaited; and if
  none arrives within the timeout the tool reports plainly that the server has not
  published diagnostics for this file yet. This is the deliberate answer to
  "`publishDiagnostics` may arrive before the tool is called".

**Pull decoding.** The `textDocument/diagnostic` result is
`DocumentDiagnosticReportResult`; a `Report(DocumentDiagnosticReport::Full(..))`
yields the `items` (the `Vec<Diagnostic>`); an `Unchanged` variant is reported as
"no change since the previous request"; `related_documents` is ignored in this
file-scoped tool. Pushed diagnostics decode from `PublishDiagnosticsParams` as
`Vec<Diagnostic>`.

Because some servers gate the pull mechanism on the client advertising
`textDocument.diagnostic` in `initialize`, `initialize_params` is extended (in the
message-builder step) to advertise minimal client support for the new features
(`references`, `hover`, `documentSymbol`, `publishDiagnostics`, `diagnostic`, and
`workspace.symbol`), each as `Some(Default::default())`, so a real server enables
them. This is a small, additive change to the existing default `ClientCapabilities`.

### Arguments

All argument types derive `serde::Deserialize` + `rmcp::schemars::JsonSchema`
(`#[schemars(crate = "rmcp::schemars")]`) with `#[schemars(description = "...")]` on
every field, so the advertised `inputSchema` is descriptive:

- **`references`** — a dedicated `ReferencesArgs` that **composes the shared
  `PositionArgs`** (`#[serde(flatten)]`) and adds `include_declaration: Option<bool>`
  (absent means include the declaration). Composition reuses the 1-based
  position contract and its `validate_position` without duplicating fields.
- **`hover`** — reuses `PositionArgs` directly (no extra fields).
- **`workspaceSymbol`** — a new `WorkspaceSymbolArgs { query: String }`.
- **`documentSymbol`** and **`diagnostics`** — a new file-only
  `FileArgs { file: String }` (one shared type, or one per tool; a single
  `FileArgs` is preferred for consistency).

Position-based tools open the document on demand through `App::open_document` and
validate with the shared `validate_position` (R4), exactly as `definition` does.

### Results

Every tool returns `Ok(success_result(text, structured))` — a human-readable text
block plus `structured_content` — carrying the raw LSP payload, and maps expected
failures to `Ok(error_result(..))` (`isError = true`) via the existing helpers.
Positions in results follow the **1-based** convention via `LocationView`/`context_for`:

- **`references`** — `{ locations: LocationView[], raw }`, the same shape as
  `definition`; each location carries URI, 1-based line/column, range end, and
  surrounding code context. `Location[]` decodes directly.
- **`hover`** — `{ uri, hover: { contents: String, range: Option<Range-as-1-based> }, raw }`.
  `contents` renders `HoverContents` (`Scalar`/`Array(MarkedString)`, `Markup`
  `MarkupContent`) to text — joining marked strings with newlines and using a
  `MarkupContent`'s `value`; the optional `range` is rendered 1-based.
- **`documentSymbol`** — `{ symbols: SymbolView[], raw }`. `DocumentSymbolResponse`
  is `Flat(Vec<SymbolInformation>) | Nested(Vec<DocumentSymbol>)`; the nested form is
  walked recursively (children included) and flattened. Each `SymbolView` carries
  `name`, `kind` (rendered as a readable name), `detail`/`containerName` where
  present, and a `LocationView` for the symbol's selection range, with context read
  from the just-opened file.
- **`workspaceSymbol`** — `{ symbols: SymbolView[], raw }`. `WorkspaceSymbolResponse`
  is `Flat(Vec<SymbolInformation>) | Nested(Vec<WorkspaceSymbol>)`. A workspace
  symbol's location is a **URI** (possibly a bare `WorkspaceLocation` with no range),
  so the code context is read on demand from that file (best-effort: an unreadable
  file yields an empty context, never an error). Symbols without a range carry the
  URI only.
- **`diagnostics`** — `{ mechanism: "pull" | "publish", diagnostics: DiagnosticView[], raw }`.
  Each `DiagnosticView` carries `severity`, `code`, `source`, `message`, and a
  `LocationView` for the diagnostic's range, with context read from the file.

A shared `SymbolView` (name/kind/detail/container/location) in `src/tools/mod.rs`
keeps the two symbol tools consistent.

### Testing strategy

Same shape as the done CR: one process-level end-to-end test over real MCP stdio,
plus fast in-process unit tests for the pure decoders/renderers.

- **Unit tests** in each new module: the `lsp-types` decoders (references `Location[]`,
  hover `HoverContents` variants, both `DocumentSymbolResponse` and
  `WorkspaceSymbolResponse` shapes, and both diagnostics reports), the text
  renderers, and the capability-gated error messages; `DiagnosticsCache` gets tests
  for storing a decoded publish, replacing per URI, and `await_newer` returning a
  fresh report / timing out; `src/lsp/message.rs` gets serialized-field-name tests
  for each new builder.
- **End-to-end (`tests/mcp.rs`)** — `McpClient::spawn` is generalized to pass extra
  server arguments after `--`, and the fake server (`src/bin/fake-lsp-server.rs`)
  is extended to advertise the new capabilities and answer
  `textDocument/references`, `textDocument/hover`, `textDocument/documentSymbol`,
  and `workspace/symbol` with canned, deterministic results. Diagnostics is covered
  in **two modes**, selected by an argument the test forwards to the fake server:
  a **pull** mode (advertise `diagnosticProvider`, answer `textDocument/diagnostic`)
  and a **push** mode (advertise no `diagnosticProvider`, send
  `textDocument/publishDiagnostics` in response to `textDocument/didOpen`). Tests
  assert `tools/list` lists all six tools with descriptions and schemas, that each
  tool returns a structured result with URI/line/column/context, and that an
  unsupported capability (a server advertising none of them) yields an `isError`
  result, all while stdout carries only MCP messages.

### Docs

`docs/architecture.md` is updated by the doc step below. The
`docs/dev/changelog.md` entry is **not** written here: that is the implementation
step's job (`fawi-implement`), not this plan's.

## Steps

- [x] **1. Add the request builders and client capabilities
      (`src/lsp/message.rs`).** Add constants for `textDocument/references`,
      `textDocument/hover`, `textDocument/documentSymbol`, `workspace/symbol`,
      `textDocument/diagnostic`, and `textDocument/publishDiagnostics`, and the
      builders `reference_params`, `document_symbol_params`,
      `workspace_symbol_params`, and `document_diagnostic_params` beside the
      existing positional builder. Extend `initialize_params` to advertise minimal
      client support (`references`, `hover`, `documentSymbol`,
      `publishDiagnostics`, `diagnostic`, `workspace.symbol`) so real servers enable
      the features. Unit-test the serialized field names of each builder. _(R6–R10;
      shared by every new tool.)_
- [x] **2. Add the shared argument and result pieces (`src/tools/mod.rs`).**
      Register the five new submodules; add `ReferencesArgs` (flattened
      `PositionArgs` + `include_declaration`), `WorkspaceSymbolArgs`, and the
      file-only `FileArgs`; add the shared `SymbolView` used by both symbol tools.
      Unit-test argument/schema composition. _(R6–R10, R15; reused by all five
      tools.)_
- [x] **3. Add capability helpers to `src/app.rs`.** `supports_references`,
      `supports_hover`, `supports_document_symbols`, `supports_workspace_symbols`,
      and `supports_pull_diagnostics`, mirroring `supports_definition`. Unit-test
      each against a synthetic `ServerCapabilities`. _(R20; the clear-error contract.)_
- [x] **4. Build the diagnostics cache (`src/diagnostics.rs`).** `DiagnosticsCache`
      subscribing to the client's notification broadcast at startup, decoding
      `textDocument/publishDiagnostics` into per-URI `Vec<Diagnostic>` with revision
      stamps, and exposing `await_newer(uri, baseline, timeout)`; wire it into `App`
      (owned `Arc`, created in `App::new`, exposed via `diagnostics()`). Unit-test
      storing/replacing per URI and the await/timeout behaviour. _(R10, R15; unblocks
      the diagnostics tool's push path.)_
- [x] **5. Implement the `references` tool (`src/tools/references.rs`).** Gate on
      `supports_references`, open the document on demand, validate the position,
      send `textDocument/references` with `includeDeclaration`, decode `Location[]`,
      and return `LocationView`s with context and the raw payload.
      _(R6, R4, R15, R16, R20; ACs references + structured result + errors.)_
- [x] **6. Implement the `hover` tool (`src/tools/hover.rs`).** Gate on
      `supports_hover`, open/validate, send `textDocument/hover`, render
      `HoverContents` and the optional range, and return the structured result with
      the raw payload. _(R7, R4, R15, R16, R20; AC hover.)_
- [x] **7. Implement the `documentSymbol` tool (`src/tools/document_symbol.rs`).**
      Gate on `supports_document_symbols`, open the file, send
      `textDocument/documentSymbol`, decode both `DocumentSymbolResponse` shapes
      (flattening the hierarchical form), and return `SymbolView`s with context.
      _(R8, R4, R15, R16, R20; AC document symbols.)_
- [x] **8. Implement the `workspaceSymbol` tool (`src/tools/workspace_symbol.rs`).**
      Gate on `supports_workspace_symbols`, send `workspace/symbol` with the query,
      decode both `WorkspaceSymbolResponse` shapes, and read each symbol's file on
      demand for context (best-effort). _(R9, R15, R16, R20; AC workspace symbols.)_
- [x] **9. Implement the `diagnostics` tool (`src/tools/diagnostics.rs`).** Gate the
      pull path on `supports_pull_diagnostics`, send `textDocument/diagnostic`, and
      decode `DocumentDiagnosticReportResult`; otherwise open the file, read the
      push cache via `await_newer`, and report clearly when nothing has arrived.
      Return `DiagnosticView`s with context, the chosen mechanism, and the raw
      payload. _(R10, R4, R15, R16, R20; AC diagnostics.)_
- [x] **10. Register the tools (`src/mcp/server.rs`).** Add the five thin `#[tool]`
      methods (name, precise description, shared `Parameters<..>` argument) delegating
      to `tools::*`, and extend `get_info`'s instructions to list all six read tools.
      _(R3, R6–R10; AC list tools with name/description/schema.)_
- [x] **11. Extend the post-handshake capability log (`src/main.rs`).** Log
      references, document symbols, workspace symbols, and pull diagnostics
      alongside the existing `definition`/`hover` flags. _(R17; observability.)_
- [x] **12. Extend the fake LSP server (`src/bin/fake-lsp-server.rs`).** Advertise
      `referencesProvider`, `documentSymbolProvider`, `workspaceSymbolProvider`;
      answer `textDocument/references`, `textDocument/hover`,
      `textDocument/documentSymbol`, and `workspace/symbol` with deterministic
      results; and select a diagnostics mode by argument — **pull** (advertise
      `diagnosticProvider`, answer `textDocument/diagnostic`) or **push** (send
      `textDocument/publishDiagnostics` after `textDocument/didOpen`). Still
      dependency-light. _(Enables the end-to-end ACs.)_
- [x] **13. Extend the end-to-end MCP tests (`tests/mcp.rs`).** Generalize
      `McpClient::spawn` to forward extra fake-server arguments; assert `tools/list`
      lists all six tools with descriptions and schemas; call each new tool and
      assert its structured result carries URI/line/column/context and the raw
      payload; cover diagnostics in **both** pull and push modes; and assert a
      capability the server does not advertise yields an `isError` result with a
      clear message. _(R6–R10, R15, R16, R17, R20; all acceptance criteria.)_
- [x] **14. Document the new tools in `docs/architecture.md`.** Extend the
      module-boundary table with `src/diagnostics.rs` and the five new `tools/`
      modules, describe the six read tools and the `App` capability gating that
      implements R20, and extend the data flow to cover the diagnostics cache
      consuming the notification broadcast (push) alongside the pull request. Keep
      the 1-based convention and stdout discipline notes current. _(R15, R17, R18,
      R20; keeps the boundary doc from drifting.)_
- [x] **15. Final verification.** Run `CARGO_NET_OFFLINE=true cargo build` and
      `CARGO_NET_OFFLINE=true cargo test` from the project root; both must pass with
      no contradictory warnings. Add **no** new dependency. The `docs/dev/changelog.md`
      entry is added by the implementation step, not this plan. _(All criteria.)_

  # Verification

  Verified in this working tree (not a git repository) on 2026-10-11T14:42:25Z:
  `CARGO_NET_OFFLINE=true cargo build` and `CARGO_NET_OFFLINE=true cargo test` both pass
  (95 unit tests, 3 CLI tests, 3 end-to-end MCP tests) with no new dependency.
