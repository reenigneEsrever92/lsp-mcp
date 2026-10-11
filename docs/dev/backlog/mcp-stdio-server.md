---
type: ChangeRequest
kind: feature
title: MCP stdio server and tool surface
description: Expose the adapter's capabilities to an MCP client over stdio with structured results.
state: done
verified: { by: maintainer, at: 2026-10-11T14:14:47Z }
priority: high
tags: [dev, mcp, tools]
owner: maintainer
---

# Problem

Even once the adapter can talk to an LSP server, an agent cannot reach any of it:
there is no MCP surface, and therefore no tools to call and no results to read.

# Proposal

Run an MCP server over stdio using `rmcp`, presenting the adapter's capabilities
as MCP tools. Each tool validates its arguments, invokes the relevant LSP client
operation, and returns a structured, agent-friendly result: the document URI,
line, and column plus the surrounding code context, alongside the raw LSP
payload where it adds fidelity. Errors are returned as actionable tool results
rather than crashes. stdout stays dedicated to the MCP transport; logs go to
stderr.

# Decisions

- **`rmcp`** — the official Rust MCP SDK, providing the stdio server transport
  and tool registration; chosen over hand-rolling the MCP JSON-RPC layer.
- **Structured results with raw payload included where useful** — so an agent can
  act on results without parsing LSP wire format, while still having the full
  response when it needs it. Chosen over raw LSP passthrough.
- **Tool results carry errors as data** — validation and LSP failures are
  reported to the client as tool errors with clear messages.
- **stdout is reserved for MCP; logs go to stderr** — required for a
  well-behaved stdio transport.
- **Scope: state and tool infrastructure plus one pilot tool** — this CR builds the
  reusable MCP server plumbing and tool infrastructure (the shared application
  state, argument/position/URI conversion, structured-result and error helpers, and
  code-context rendering) and registers a single pilot tool, `definition`, to prove
  the surface end to end. The remaining read/explore tools (references, hover,
  document symbols, workspace symbols, diagnostics) land in
  [Read/explore tools](read-explore-tools.md), and the action tools in
  [Code-action tools](code-action-tools.md) and
  [Write tools: rename and format](write-tools-rename-format.md). Those requests
  reuse this CR's infrastructure rather than re-planning it.

# Acceptance criteria

- An MCP client connected over stdio can list the adapter's tools.
- Each registered tool reports a name, description, and input schema.
- Calling a tool returns a structured result containing document locations in
  agent-friendly form.
- A tool call with invalid arguments returns a clear error result, not a crash.
- The process writes only MCP messages to stdout.

# Implementation plan

## Approach

This lands as one coherent deliverable — the MCP server plumbing and reusable tool
infrastructure, plus a single pilot tool — so it is **not** decomposed into
sub-tasks. The remaining tools reuse the same shared pieces, so getting the shape
of those pieces right here is the point of the CR: the later requests
([Read/explore tools](read-explore-tools.md), [Code-action
tools](code-action-tools.md), [Write tools: rename and
format](write-tools-rename-format.md)) register more tools against this
infrastructure rather than reinventing it. R18's boundaries are extended, not
redrawn: `lsp/` still knows nothing about MCP, and a new `mcp/` (server) and
`tools/` (tool layer) sit above it, wired only by `main.rs`.

### Module layout

- `src/main.rs` — entry point. Stops exiting after the handshake: it initialises
  `tracing` to **stderr**, parses the CLI, spawns the LSP client (the handshake
  already runs inside `Client::spawn`), builds the shared `App`, serves MCP over
  stdio, and waits for the transport to close. On an error it prints to stderr and
  exits non-zero, without a panic.
- `src/app.rs` — **the shared application state**. `App` owns the one `lsp::Client`
  created at startup and the on-demand document store, and is the only thing the
  tools and the server share. `Client::request(&self, ..)` already takes `&self`, so
  the client is wrapped once in an `Arc`; `Documents::ensure_open`/`close` take
  `&mut self`, so the store sits behind a `tokio::sync::Mutex` (the tools are async).
  `App` also exposes the small helpers the tool layer needs: capability lookup, and
  an "open this file for me" helper that reads the path from disk and calls
  `Documents::ensure_open`. Both the MCP server and every tool hold an `Arc<App>`.
- `src/mcp/mod.rs` + `src/mcp/server.rs` — the MCP server. `LspServer` holds
  `Arc<App>` and the `ToolRouter<Self>`; `#[tool_router]` collects the `#[tool]`
  methods (initially just `definition`), `#[tool_handler(router = self.tool_router)]`
  fills in `call_tool`/`list_tools`, and `get_info()` advertises tool support via
  `ServerCapabilities::builder().enable_tools().build()`. A `serve`/`run` function
  builds the stdio transport (`rmcp::transport::stdio()`) and drives the service.
- `src/tools/mod.rs` — **the reusable tool infrastructure** (see below).
- `src/tools/definition.rs` — the one pilot tool, mapping `textDocument/definition`
  to a structured result.

The `#[tool]` methods on `LspServer` stay thin: they parse the shared argument type
and delegate to the matching function in `tools/`, so tool behaviour lives in the
tool layer, not in the server module.

### Shared state and startup

The LSP client is created **once** at startup, exactly as today, and handed to
`App`. Nothing in the tool layer spawns a process or performs a handshake; the
plumbing that does (the current `main::run`) moves into the serve path. Dropping the
`Client` still aborts its tasks and kills the child, so the single `Arc<App>` keeps
the server alive for the whole MCP session and tears it down on exit.

### Shared argument types and position/URI conversion

The tool layer defines one reusable argument shape for position-based tools (this
CR's `definition`, and later `references`, `hover`, `codeAction`, `rename`): a file
path plus a line and a column, deriving `serde::Deserialize` and
`rmcp::schemars::JsonSchema` (both re-exported by `rmcp`) with
`#[schemars(description = "...")]` on each field so the advertised `inputSchema` is
descriptive. Conversion and validation live alongside it:

- **Position base — decided:** tool arguments take a **1-based line and column**
  (what a user sees in an editor and an agent most naturally reasons about) and are
  converted to a **0-based `lsp_types::Position`** before the request. The base is
  stated in the field descriptions and enforced by validation; it is documented in
  `docs/architecture.md` so later tools inherit the same convention.
- **Path → URI:** reuse the existing `lsp::path_to_uri` to build the
  `lsp_types::Uri`, and the `TextDocumentIdentifier`/`TextDocumentPositionParams`
  builders in `src/lsp/message.rs` (add the missing positional builder there, next
  to the `didOpen`/`didChange`/`didClose` ones).
- **Validation:** reject a missing/empty path, a line or column of `0` under the
  1-based convention, and a position past the end of the file (checked against the
  text the helper already read to open the document), each with a clear message.

### Structured results, code context, and raw payload

The tool layer provides result helpers so every tool returns the same agent-friendly
shape (R15):

- a **`CallToolResult`** carrying both a human-readable text block and
  `structured_content` built from a small serializable location type — document
  URI, 1-based line and column, the range end, and the surrounding **code context**;
- a **code-context renderer** that reads the file and prints the surrounding lines
  with line numbers and the target range marked, so an agent does not need a second
  call to see what is at a location;
- the **raw LSP payload** included alongside, where it adds fidelity.

### Errors as data, never panics

A single conversion maps a `crate::error::Error` (and any validation failure) to
`Ok(CallToolResult::error(..))` with `is_error = true` — a _tool-level_ error the
client renders and the agent can act on — rather than a JSON-RPC protocol error
that a client hides. Tools return `Result<CallToolResult, rmcp::ErrorData>` so the
handler can choose `Ok(CallToolResult::error(..))` for expected failures; the result
shape is chosen over `Json<T>` precisely so the text block and the structured block
can both be controlled. A server that does not advertise a capability the tool needs
is reported as a clear tool error (R20). No `unwrap`/`expect` on any request path.

### stdout discipline

stdout belongs exclusively to `rmcp`'s transport; the transport is
**newline-delimited JSON-RPC** (one JSON object per line), which is what MCP over
stdio specifies. `tracing` stays configured to stderr, no `println!` is introduced,
and the spawned LSP server's stderr is still inherited (R17). The end-to-end test
below pins this by asserting every non-empty stdout line is a JSON-RPC message.

### Technology: `rmcp`

Add `rmcp` at the cached `3.5` (resolving to `3.5.1`) with the `transport-io`
feature; its default features (`server`, `macros`, and `schemars` via `server`) are
required. `transport-io` activates `tokio/io-std` transitively, so `stdio()` works;
add `io-std` to the crate's own `tokio` feature list as well for explicitness. No
other crate is added: `rmcp` re-exports `schemars`, `serde`, and `serde_json`, so the
tool argument derives use `rmcp::schemars`/`rmcp::serde`. Builds and tests run as
`CARGO_NET_OFFLINE=true cargo build` and `CARGO_NET_OFFLINE=true cargo test`.

### Testing strategy — one primary approach, plus fast unit tests

**Primary: a process-level end-to-end test that speaks real MCP over stdio.** A new
`tests/mcp.rs` spawns the compiled `lsp-mcp` binary with
`-- <CARGO_BIN_EXE_fake-lsp-server>` and acts as an MCP client over the child's
stdin/stdout, writing and reading **newline-delimited** JSON-RPC. This is the right
primary because the acceptance criteria are process-level contracts — list tools,
structured result, error-as-data with invalid arguments, and stdout-only-MCP — and
this is the only approach that exercises the real CLI wiring, the real stdio
transport, and the stdout contract together. The existing `tests/cli.rs` already
proves the process harness and the `CARGO_BIN_EXE_*` lookup, and the existing
`fake-lsp-server` bin already exists, so the marginal cost is small.

The **alternative considered** — driving `LspServer` in-process over a
`tokio::io::duplex` pair with hand-written JSON-RPC (as `rmcp`'s own tests do) — is
rejected as the primary because the server owns a _real_ child LSP process through
`App` regardless, so an in-process server still needs the fake LSP binary, while it
would not prove the CLI wiring or the stdout contract. `rmcp`'s duplex style may
still be used for narrow server tests if useful, but it is not the backbone.

**Plus: in-process unit tests for the pure helpers.** The argument/position/URI
conversion and validation, the code-context renderer, and the error→result mapping
are pure functions tested directly, with no transport.

Because the pilot tool renders context from the file on disk and echoes the location
the server returns, the fake LSP server is extended to answer
`textDocument/definition` with a real `Location` (echoing the requested document URI
at a fixed position), and the end-to-end test writes a temporary file whose contents
it then expects to see in the context fields.

### Docs

`docs/architecture.md` gains the new `mcp/` and `tools/` modules, the `App` state,
the MCP data flow, the newline-delimited stdio contract, and the 1-based
line/column convention. The `docs/dev/changelog.md` entry is **not** written here:
that is the implementation step's job (`fawi-implement`), not this plan's.

## Steps

- [x] **1. Add the `rmcp` dependency.** Add `rmcp = { version = "3.5", features =
["transport-io"] }` (defaults `server`, `macros`, `schemars` retained) and
      `"io-std"` to the `tokio` feature list in `Cargo.toml`. Verify
      `CARGO_NET_OFFLINE=true cargo build` still succeeds. _(R3; unblocks every step.)_
- [x] **2. Introduce the shared application state (`src/app.rs`).** `App` owning
      `Arc<Client<ChildStdout, ChildStdin>>` and a `tokio::sync::Mutex<Documents>`,
      with a constructor from the spawned client, a capability lookup, and an
      open-document helper that reads a path from disk and calls `ensure_open`.
      Register `mod app;` in `src/main.rs`. _(R3, R4, R18; the state R18 keeps behind
      one boundary and every later tool reuses.)_
- [x] **3. Add the positional request/notification builders to
      `src/lsp/message.rs`.** A builder for `textDocument`-position params
      (`TextDocumentIdentifier` + `Position`) to sit beside the existing
      `didOpen`/`didChange`/`didClose` builders. Unit-test the serialized field
      names. _(R4; shared by all position-based tools.)_
- [x] **4. Build the shared tool infrastructure (`src/tools/mod.rs`).** The reusable
      position argument type (file + 1-based line/column, `Deserialize` +
      `JsonSchema` with field `#[schemars(description = "...")]`), the
      position/URI conversion and validation (via `lsp::path_to_uri` and the message
      builders), the structured-result type and builders (`CallToolResult` with a
      text block plus `structured_content`), the code-context renderer, and the
      `crate::error::Error` → `Ok(CallToolResult::error(..))` conversion. Unit-test
      the conversions, validation edge cases (line/column `0`, out-of-range, missing
      path), the context renderer, and the error mapping. _(R15, R16, R18, R20; ACs
      structured result + invalid arguments.)_
- [x] **5. Implement the pilot `definition` tool (`src/tools/definition.rs`).**
      Validate the arguments, open the document on demand through `App`, send
      `textDocument/definition`, decode `GotoDefinitionResponse`
      (`Scalar`/`Array`/`Link`), and return a structured result with each location's
      URI, 1-based line/column, surrounding code context, and the raw payload. A
      server that omits `definitionProvider`, and a position with no definition, both
      yield a clear tool result. _(R5, R4, R15, R16, R20; ACs structured result,
      invalid arguments, capability reporting.)_
- [x] **6. Build the MCP server (`src/mcp/mod.rs`, `src/mcp/server.rs`).** `LspServer
{ app: Arc<App>, tool_router: ToolRouter<Self> }`; a `#[tool_router]` impl
      registering `definition` as a thin `#[tool(name = "definition",
description = "...")]` method that delegates to `tools::definition`;
      `#[tool_handler(router = self.tool_router)] impl ServerHandler` with `get_info()`
      returning `ServerConfig::new(ServerCapabilities::builder().enable_tools().build())`;
      and a `serve`/`run` function that builds `rmcp::transport::stdio()` and awaits
      the running service. _(R3, R5; ACs list tools with name/description/schema.)_
- [x] **7. Rewire `src/main.rs`.** Keep `tracing` on **stderr**; parse the CLI, spawn
      the client (handshake inside `Client::spawn`), build the `App`, serve MCP over
      stdio, and wait for the transport to close; on error print to stderr and exit
      non-zero. Remove the "handshake then exit" behaviour. _(R3, R17; ACs stdout-only
      MCP, list tools.)_
- [x] **8. Extend the fake LSP server (`src/bin/fake-lsp-server.rs`).** Answer
      `textDocument/definition` with a real `Location` that echoes the requested
      document URI at a fixed position (still dependency-light, reusing `serde_json`),
      so the end-to-end test can assert a concrete URI/line/column/context. It already
      ignores notifications, so `didOpen`/`initialized` need no change. _(Enables the
      AC "structured result containing document locations".)_
- [x] **9. Add the end-to-end MCP tests (`tests/mcp.rs`).** Spawn the compiled
      `lsp-mcp` with `-- <CARGO_BIN_EXE_fake-lsp-server>` and speak newline-delimited
      JSON-RPC over its stdio: complete `initialize` + `notifications/initialized`;
      assert `tools/list` returns `definition` with a description and an input schema;
      call `definition` on a temp file written by the test and assert the structured
      result carries the URI, line, column, and the expected context; call it with
      invalid arguments (bad position / missing file) and assert a tool error result
      (`isError = true`) rather than a crash; and assert every non-empty stdout line
      is a JSON-RPC message while logs appear on stderr. _(R3, R5, R15, R16, R17;
      all acceptance criteria.)_
- [x] **10. Document the MCP layer in `docs/architecture.md`.** Add the `mcp/`
      (`LspServer`/router/`get_info`/serve) and `tools/` modules and the `App` state to
      the module-boundary table; extend the data flow to cover `main → App →
LspServer → tools → Client → LSP server`; document the newline-delimited stdio
      contract and the 1-based line/column convention the shared argument types
      enforce; note that stdout stays exclusively MCP. _(R17, R18; keeps the boundary
      doc from drifting, the R18 deliverable.)_
- [x] **11. Final verification.** Run `CARGO_NET_OFFLINE=true cargo build` and
      `CARGO_NET_OFFLINE=true cargo test` from the project root; both must pass with no
      contradictory warnings. The `docs/dev/changelog.md` entry is added by the
      implementation step, not this plan. _(All criteria.)_

Verified in this working tree: `CARGO_NET_OFFLINE=true cargo build` and
`CARGO_NET_OFFLINE=true cargo test` both pass (62 tests: 58 unit, 3 CLI, 1
end-to-end MCP), with no compiler warnings.
