---
type: Requirements
title: Requirements
description: The users, use cases, technology choices, and numbered requirements for lsp-mcp.
tags: [requirements, lsp, mcp]
status: active
---

# Requirements

This document records the analysis behind `lsp-mcp`: who it is for, what they
will do with it, how it will be built, and the numbered requirements that follow.
See the [overview](overview.md) for the project summary.

## Users

| ID  | Persona                                                                     | Goal                                                                                                         | Expertise                   | Cares about                                                                           | Class       |
| --- | --------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------ | --------------------------- | ------------------------------------------------------------------------------------- | ----------- |
| U1  | **Agent user** — a software engineer working through an MCP client or agent | Let their agent explore a codebase and make changes using real language intelligence rather than text search | High; reads and writes code | Correctness, structured results the agent can act on, low latency, server-agnosticism | **Primary** |

No secondary users are in scope.

## Use cases

Each use case is stated concretely enough to derive an acceptance criterion. All
belong to U1.

### Explore / read

- **UC1 Definition** — given a file and a position, the agent gets the location
  where the symbol at that position is defined.
- **UC2 References** — given a file and a position, the agent gets every
  reference to the symbol at that position.
- **UC3 Hover** — given a file and a position, the agent gets the type,
  signature, and documentation of the symbol there.
- **UC4 Document symbols** — given a file, the agent gets the symbols it
  contains.
- **UC5 Workspace symbols** — given a query string, the agent gets matching
  symbols across the workspace.
- **UC6 Diagnostics** — given a file (or the whole workspace), the agent gets the
  diagnostics reported for it.

### Act / modify

- **UC7 Code actions** — given a file and a position (and optionally the
  diagnostics there), the agent lists the code actions that apply and can apply
  one.
- **UC8 Rename** — given a file and a position plus a new name, the agent renames
  the symbol there. This writes files.
- **UC9 Format** — given a file (or a range within it), the agent formats it.
  This writes files.

## Technology choices

| Concern        | Choice                                                      | Alternative considered                 | Why the choice won                                                                                                                                                                                                         |
| -------------- | ----------------------------------------------------------- | -------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Language       | **Rust**                                                    | TypeScript/Node, Go                    | A single self-contained binary; strong async and child-process support                                                                                                                                                     |
| Async runtime  | **`tokio`**                                                 | `async-std`, OS threads                | The de-facto standard; the best process and stdio support                                                                                                                                                                  |
| CLI parsing    | **`clap`** (derive)                                         | `argh`, hand-rolled                    | Clean handling of `--` argument passthrough and `--help`                                                                                                                                                                   |
| LSP types      | **`lsp-types`**                                             | hand-rolled structs                    | Canonical, complete definitions of the protocol's messages                                                                                                                                                                 |
| LSP transport  | **Hand-rolled JSON-RPC client** over `tokio::process` stdio | `async-lsp`, `lsp-server`, `tower-lsp` | The adapter must be server-agnostic, pass arbitrary and server-specific methods through, and own per-request timeouts — all of which need the raw message stream. Those crates are server-first and fight the client role. |
| MCP server     | **`rmcp`**                                                  | hand-rolled JSON-RPC                   | The official Rust MCP SDK; provides the stdio server transport and tool registration                                                                                                                                       |
| Serialization  | **`serde` + `serde_json`**                                  | —                                      | Required by both protocol layers                                                                                                                                                                                           |
| Error handling | **`anyhow` + `thiserror`**                                  | —                                      | Context at boundaries, typed errors within the adapter                                                                                                                                                                     |
| Logging        | **`tracing`**, written to stderr                            | `log`/`env_logger`                     | stdout is owned by the MCP transport, so logs must go to stderr                                                                                                                                                            |
| File URIs      | **`url`**                                                   | —                                      | `lsp-types` represents document locations as `Url`                                                                                                                                                                         |

## Requirements

Requirements are traced to the user and use case that produced them. Priorities
use the same `low`/`medium`/`high` scale as the backlog.

### Functional

| ID  | Requirement                                                                                                                                                               | Traces to | Priority |
| --- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------- | -------- |
| R1  | The CLI binary spawns a configured LSP server and forwards arguments after `--` to it                                                                                     | U1        | High     |
| R2  | The adapter speaks the LSP client protocol over the child process's stdio: `Content-Length` framing, the `initialize` handshake, and capture of the server's capabilities | U1        | High     |
| R3  | The adapter exposes an MCP server over stdio to its parent process                                                                                                        | U1        | High     |
| R4  | Documents are opened and updated on demand (`didOpen`/`didChange`/`didClose`) before any position-based request                                                           | UC1–UC9   | High     |
| R5  | A definition tool resolves the symbol at a position to its definition location                                                                                            | UC1       | High     |
| R6  | A references tool lists every reference to the symbol at a position                                                                                                       | UC2       | High     |
| R7  | A hover tool returns the type, signature, and documentation at a position                                                                                                 | UC3       | High     |
| R8  | A document-symbols tool lists the symbols in a file                                                                                                                       | UC4       | High     |
| R9  | A workspace-symbols tool searches symbols across the workspace                                                                                                            | UC5       | High     |
| R10 | A diagnostics tool returns diagnostics for a file or the workspace                                                                                                        | UC6       | High     |
| R11 | A tool lists the code actions that apply at a position                                                                                                                    | UC7       | Medium   |
| R12 | A tool applies or executes a chosen code action                                                                                                                           | UC7       | Medium   |
| R13 | A rename tool renames the symbol at a position                                                                                                                            | UC8       | Medium   |
| R14 | A format tool formats a file or a range                                                                                                                                   | UC9       | Medium   |
| R15 | Results are structured and agent-friendly (document URI, line, column, and surrounding code context) and include the raw LSP payload where it is useful                   | UC1–UC9   | High     |

### Non-functional

| ID  | Requirement                                                                                                                                                                                 | Priority |
| --- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------- |
| R16 | Reliability: server missing or crashed, document not yet opened, file missing on disk, out-of-range position, request timeout, and LSP error responses all produce clean, actionable errors | High     |
| R17 | Observability: all logs are written to stderr; stdout is reserved exclusively for the MCP transport                                                                                         | High     |
| R18 | Maintainability: clear module boundaries between the LSP client, the MCP server, the tool layer, and configuration                                                                          | Medium   |
| R19 | Portability: shipped as a single binary with no runtime dependencies                                                                                                                        | Medium   |
| R20 | Server-agnosticism: no server is hardcoded; behaviour is driven by the capabilities the spawned server advertises                                                                           | High     |

## Initial milestone

The initial milestone (**M1**) was the smallest coherent slice that demonstrates
the project's value: a working adapter that spawns an LSP server, speaks MCP over
stdio to the parent, and exposes the full **read/explore** tool set end to end.
It covers **R1–R10** and **R15–R20**.

The **act/modify** requirements — **R11–R14** (code actions, rename, format) —
were delivered as the immediate follow-on milestone **M2**, so that the
foundation plus read tools landed and proved out first. They were tracked by
their own change requests rather than folded into M1. Both milestones have now
shipped, so the whole first release (**R1–R20**) is implemented.

See the [backlog](dev/backlog/index.md) for the change requests that implement
these requirements, and the [changelog](dev/changelog.md) for what has shipped.
