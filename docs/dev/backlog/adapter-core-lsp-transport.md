---
type: ChangeRequest
kind: feature
title: Adapter core, CLI, and LSP client transport
description: Spawn an LSP server, speak the client protocol over its stdio, and manage the session lifecycle.
state: done
priority: high
tags: [dev, lsp, cli, transport]
owner: maintainer
verified: { by: maintainer, at: 2026-10-11T13:57:13Z }
---

# Problem

There is no adapter at all. Nothing can launch an LSP server, talk to it, or
manage its lifecycle, so no other capability has a foundation to build on.

# Proposal

Create the binary and its command-line interface, then the LSP client beneath it.
The CLI takes a server command to launch and forwards everything after `--` to
that server. The client spawns the server with `tokio::process` and speaks
hand-rolled JSON-RPC 2.0 over the child's stdio using `Content-Length` framing.
It performs the `initialize`/`initialized` handshake, captures and stores the
server's advertised capabilities, and manages documents on demand
(`didOpen`/`didChange`/`didClose`) so position-based requests can be served.
Each request gets a timeout, server exit is detected and surfaced, and transport
failures become typed errors the tool layer can convert into clean messages.

# Decisions

- **Rust with `tokio`** — a single self-contained binary with first-class child
  process and stdio support; chosen over TypeScript/Node and Go.
- **`clap` (derive) for the CLI** — handles `--` argument passthrough and
  `--help` cleanly; chosen over `argh` and hand-rolling.
- **Hand-rolled JSON-RPC client over `tokio::process`** — the adapter must be
  server-agnostic, forward arbitrary and server-specific methods, and own
  per-request timeouts, all of which need the raw message stream. Rejected
  `async-lsp`, `lsp-server`, and `tower-lsp` because they are server-first and
  fight the client role.
- **`lsp-types` for message shapes** — canonical protocol definitions; chosen
  over hand-rolled structs.
- **`anyhow` + `thiserror`** — `thiserror` for typed transport errors, `anyhow`
  for context at the boundaries.
- **`tracing` to stderr** — stdout is reserved for the MCP transport, so all
  diagnostics go to stderr.
- **`url`** — `lsp-types` represents document locations as `Url`.

# Acceptance criteria

- Running the binary with a server command and arguments after `--` spawns that
  server and completes the `initialize` handshake; the server's capabilities are
  captured.
- After opening a document, a position-based request succeeds against a real
  server.
- Framing is correct under partial reads and headers split across chunks.
- A missing or unlaunchable server produces a clean error and a non-zero exit,
  not a panic.
- A server that exits mid-session is detected and reported as an error.
- A request that exceeds its timeout fails cleanly rather than hanging.
- All log output goes to stderr; stdout carries nothing but the MCP transport.

# Implementation plan

## Approach

This lands as a single binary crate — no workspace — because the foundation is one
coherent deliverable that later change requests extend rather than replace. The
crate is `lsp-mcp` with a single `[[bin]]` target; everything the MCP server and the
tool layer will reuse lives behind two public module boundaries: `lsp::Client`
(spawn, handshake, capability capture, correlated requests/notifications, timeouts,
exit detection) and `lsp::Documents` (the on-demand `didOpen`/`didChange`/`didClose`
lifecycle). Nothing under `lsp/` knows about MCP; `main.rs` is the only place that
wires the CLI and process lifecycle to the client, which keeps R18's boundaries
intact for the CRs that follow.

**Module layout.**

- `src/main.rs` — entry point: initialize `tracing` to stderr, parse the CLI, spawn
  and hand `initialize` the server, log a one-line summary, exit. In this CR there
  is no MCP loop yet, so a successful run performs the handshake and exits `0`.
- `src/cli.rs` — `clap` (derive) argument parsing for `lsp-mcp -- <server-command>
[args...]`, capturing the program and its arguments as an ordered `Vec<String>`.
- `src/error.rs` — a `thiserror` `Error` enum (`SpawnFailed`, `Io`, `Framing`,
  `Protocol`, `JsonRpc` (LSP error responses), `Timeout`, `ServerExited`) plus a
  crate `Result` alias; `anyhow` supplies context only at the `main` boundary.
- `src/lsp/mod.rs` — re-exports the submodules below.
- `src/lsp/transport.rs` — `Content-Length` framing, generic over `AsyncRead` /
  `AsyncWrite`, with `read_frame`/`write_frame`; also the `spawn(command, args)`
  constructor that wires `tokio::process::Child` stdio into the same framing.
- `src/lsp/message.rs` — JSON-RPC 2.0 envelope types (`Id`, `Request`, `Response`,
  `Notification`, `ErrorObject`) with `serde`, plus thin builders that derive
  request/notification payloads from `lsp_types` (`Initialize`, `Initialized`,
  `DidOpenTextDocument`, `DidChangeTextDocument`, `DidCloseTextDocument`, …).
- `src/lsp/client.rs` — `Client<R, W>`: the background reader task and writer task,
  the `initialize`/`initialized` handshake and captured `ServerCapabilities`,
  request/notification correlation, per-request timeouts, and exit detection.
- `src/lsp/documents.rs` — `Documents`: per-URI version and text bookkeeping and the
  `didOpen`/`didChange`/`didClose` notifications, with `ensure_open(path, text)`.

**Genericity for testability.** `Client` and the framing layer are generic over
`R: AsyncRead + Unpin + Send` and `W: AsyncWrite + Unpin + Send`. `Client::spawn`
returns `Client<ChildStdout, ChildStdin>`; tests construct the same `Client` over a
`tokio::io::duplex` pair, so every behavioural acceptance criterion below is
exercised by an in-process fake LSP server with no external dependency. A single
truly end-to-end CLI test exercises the compiled binary (see step 11).

**Concurrency model.** One reader task owns the read half: it reads framed messages
in a loop and dispatches by shape — responses resolve the pending `oneshot` keyed by
`Id`; server-to-client notifications (e.g. `window/logMessage`) are forwarded to a
broadcast channel for later CRs; server-to-client requests are answered with
`MethodNotFound` for now. One writer task owns the write half and drains an `mpsc`
channel, so frames are never interleaved. Requests allocate a monotonically
increasing `Id`, register a `oneshot` in a shared map, enqueue the frame, and await
the response under `tokio::time::timeout` (R16). When the reader hits EOF it marks
the client dead and fails every pending request with `ServerExited` (R16). Dropping
the client aborts both tasks and kills the child.

**URI note.** `lsp-types` 0.97 represents document locations as `lsp_types::Uri`
(backed by `fluent-uri`), not `url::Url`. `url::Url` (the fixed choice) is used for
the filesystem-path ↔ URI conversion at the boundary — `Url::from_file_path` and
back — and the resulting string is adapted to `lsp_types::Uri` when building
`TextDocumentIdentifier`s.

**Testing strategy.** Framing is unit-tested directly (byte-at-a-time headers,
split bodies, oversized/invalid `Content-Length`). Client behaviour is integration-
tested over a duplex against a small in-harness fake server. The one process-level
test needs a real executable to spawn, so the crate adds a second, dependency-light
bin target `src/bin/fake-lsp-server.rs` (it reuses the crate's `serde_json` and
answers `initialize`/`definition` from canned data). `cargo test` builds all bin
targets, so the integration test in `tests/cli.rs` locates both executables through
Cargo's `CARGO_BIN_EXE_<name>` environment variables — `env!("CARGO_BIN_EXE_lsp-mcp")`
and `env!("CARGO_BIN_EXE_fake-lsp-server")` — and spawns the real `lsp-mcp` binary
with `-- <fake-server>`. No network or external server is required.

**Dependencies.** Only cached versions: `tokio 1.53` (`rt-multi-thread`, `macros`,
`process`, `io-util`, `sync`, `time`), `clap 4.6` (derive), `lsp-types 0.97`,
`serde 1.0.229` (derive), `serde_json 1.0.152`, `anyhow 1.0.104`, `thiserror 2.0.21`,
`tracing 0.1.44`, `tracing-subscriber 0.3.23`, `url 2.5.8`. Builds and tests run as
`CARGO_NET_OFFLINE=true cargo build` and `CARGO_NET_OFFLINE=true cargo test`.

**Docs.** Adds `docs/architecture.md` — the module boundaries and data flow that
R18 calls for and that the later CRs build on — and links it from `docs/index.md`
(Area) and `docs/dev/index.md` (Sections). No other bundle document needs changing
here; the changelog is updated by the later implementation step, not this plan.

## Steps

- [x] **1. Scaffold the crate and pin dependencies.** Create `Cargo.toml` (package
      `lsp-mcp`, one `[[bin]]` target, Rust 2021 edition) with exactly the cached
      dependency set and features listed under _Approach → Dependencies_, and a minimal
      `src/main.rs`. Verify `CARGO_NET_OFFLINE=true cargo build` succeeds. _(Foundation
      for every criterion; R19.)_
- [x] **2. Implement the CLI (`src/cli.rs`).** `clap` (derive) args that capture the
      program and its arguments from `lsp-mcp -- <server-command> [args...]`, preserving
      order, and error when no server command is given. Unit-test parsing. _(R1; AC missing
      server.)_
- [x] **3. Implement typed errors (`src/error.rs`).** The `thiserror` `Error` enum
      and `Result` alias, with `anyhow` context confined to `main`. Unit-test that each
      variant renders a clean, actionable message. _(R16; AC missing server.)_
- [x] **4. Implement the JSON-RPC message layer (`src/lsp/message.rs`).** Envelope
      types with `serde` and the `lsp_types`-derived request/notification builders.
      Round-trip unit tests for request, response, notification, and error object.
      _(R2, R20; AC spawn + handshake.)_
- [x] **5. Implement `Content-Length` framing (`src/lsp/transport.rs`).** Generic
      `read_frame`/`write_frame` over `AsyncRead`/`AsyncWrite`. Unit-test: normal frame;
      headers delivered one byte at a time; body split across reads; missing/oversized/
      malformed `Content-Length` yields a typed `Framing` error. _(R2; AC framing under
      partial reads and split headers.)_
- [x] **6. Implement the spawn constructor.** `spawn(command, args)` building a
      `tokio::process::Command` with piped stdin/stdout, wiring the child stdio into the
      generic transport. Test that an unlaunchable command returns the typed
      `SpawnFailed` error rather than panicking. _(R1, R16, R19; AC missing/unlaunchable
      server.)_
- [x] **7. Implement the client (`src/lsp/client.rs`).** Reader/writer tasks, `Id`
      correlation via `oneshot`, `initialize`/`initialized` handshake with
      `ServerCapabilities` capture, per-request `tokio::time::timeout`, and EOF/exit
      detection that fails pending requests with `ServerExited`. _(R2, R16, R20; ACs
      handshake + capabilities, mid-session exit, timeout.)_
- [x] **8. Implement the document store (`src/lsp/documents.rs`).** Per-URI version
      and text state, `ensure_open`, and the `didOpen`/`didChange`/`didClose`
      notifications emitted through the client. Unit-test version increments and the
      notification params (full vs. incremental text). _(R4; AC position request after
      open.)_
- [x] **9. Write the client integration tests over a duplex.** An in-harness fake LSP
      server: assert the handshake and capability capture; assert a
      `textDocument/definition` request succeeds after `didOpen`; assert two in-flight
      requests correlate to the right responses; assert an over-timeout request fails
      with `Timeout`; assert a server that closes mid-session fails pending requests.
      _(R2, R4, R16; ACs position request after open, mid-session exit, timeout.)_
- [x] **10. Wire `src/main.rs`.** Initialize `tracing_subscriber` writing to
      **stderr**, parse the CLI, spawn and handshake, log a one-line summary to stderr,
      and exit `0`; on any error print the `anyhow`-contexted message to stderr and exit
      non-zero without a panic. _(R17, R16; ACs stdout-only-MCP, missing server.)_
- [x] **11. Add the process-level CLI test.** Add the second bin target
      `src/bin/fake-lsp-server.rs` and `tests/cli.rs`, which spawns the compiled `lsp-mcp`
      via `env!("CARGO_BIN_EXE_lsp-mcp")` with `-- <path from
  CARGO_BIN_EXE_fake-lsp-server>`, and asserts: the `--` passthrough launches the
      server and the handshake completes; the process's **stdout is empty** while its
      stderr carries the logs; and running with a nonexistent server command exits
      non-zero with a clean stderr message. _(R1, R16, R17; ACs spawn + handshake,
      stdout-only-MCP, missing server.)_
- [x] **12. Document the architecture.** Add `docs/architecture.md` describing the
      module boundaries (`cli`, `error`, `lsp::{transport, message, client, documents}`),
      the data flow from CLI through the client to the child's stdio, and the
      stdout/stderr contract; add an `Architecture` entry to `docs/index.md` (Areas) and
      `docs/dev/index.md` (Sections). _(R18.)_
- [x] **13. Final verification.** Run `CARGO_NET_OFFLINE=true cargo build` and
      `CARGO_NET_OFFLINE=true cargo test` from the project root; both must pass with no
      warnings that contradict the design, and a manual real-server smoke run
      (`lsp-mcp -- <a real LSP server>`) should be noted as the one check that cannot be
      automated offline. _(All criteria.)_

## Verification

Implemented and verified in the current working tree; this project is not yet
under version control, so there is no commit to link. `CARGO_NET_OFFLINE=true
cargo build` finishes with no warnings and `CARGO_NET_OFFLINE=true cargo test`
passes 40 unit tests plus 3 process-level CLI tests, with 0 failures.
