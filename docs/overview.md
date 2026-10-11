---
type: Overview
title: Overview
description: What lsp-mcp is, who it serves, and what it deliberately leaves out.
tags: [adapter, lsp, mcp, overview]
status: active
---

# Overview

`lsp-mcp` is a single-binary adapter that bridges a Language Server Protocol
(LSP) server and a Model Context Protocol (MCP) client. An agent that can only
search text is slow and imprecise at understanding code; by exposing an LSP
server's capabilities over MCP, `lsp-mcp` gives the agent precise answers —
where a symbol is defined, what references it, its type, the symbols in a file,
the diagnostics a file has, which code actions apply, and how to rename or
format code — so it can explore a codebase and make changes efficiently.

It serves the **agent user**: a software engineer working through an MCP client
or agent. The adapter is **server-agnostic** — you name an LSP server program as an argument
and forward that server's own arguments after `--`. It ships as a
single Rust binary; see the [architecture](architecture.md) for its module
boundaries and data flow.

The project deliberately excludes MCP transports other than stdio, LSP
transports other than the spawned server's stdio, multiplexing several servers,
and higher-level automated refactoring workflows — the adapter exposes LSP
primitives, not multi-step refactors.

See the [requirements](requirements.md) for the users, use cases, and technology
choices, and the [development section](dev/index.md) for how the work is
organized.
