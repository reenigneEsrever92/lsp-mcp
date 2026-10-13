---
type: ChangeRequest
kind: improvement
title: Advertise the working directory as the workspace root
description: Send the process working directory in the LSP initialize handshake as workspaceFolders and the legacy rootUri, so root-requiring servers index the project.
state: done
priority: high
tags: [dev, lsp, handshake, workspace-root]
owner: maintainer
verified: { by: maintainer, at: 2026-10-13T00:00:00Z }
---

# Problem

The adapter sends no workspace root in the LSP `initialize` handshake — both
`root_uri` and `workspace_folders` are `None`, and the process id is the only
hint. This assumes every server falls back to its own working directory when the
client sends no root. Many do (`rust-analyzer`, `gopls`, `clangd`), but not all:
`java-lsp` indexes a workspace only from `rootUri`/`workspaceFolders`, so driven
through `lsp-mcp` its index stays empty, `definition` and `references` return
nothing, and none of its project-aware features have anything to act on. The gap
is invisible in the adapter's own tests, which run against the test-only fake
server, and it contradicts the assumption recorded in
[`test-projects/README.md`](../../../test-projects/README.md).

# Proposal

Advertise the adapter's process working directory as the workspace root during the
handshake: send it as `workspaceFolders` (the modern field) and as the deprecated
`rootUri` (kept for servers that only read that field). Set neither when the
working directory cannot be resolved, leaving project selection to the server.
Also advertise the client's `workspace.workspaceFolders` capability, since the
adapter now provides folder information.

# Decisions

- **Send both `workspaceFolders` and `rootUri`.** `rootUri` is deprecated but is
  still the field some servers read first (it is the one `java-lsp` consults
  before `workspaceFolders`); sending both maximises compatibility at no cost,
  since they carry the same directory.
- **The process working directory is the root.** The adapter is server-agnostic
  and owns no project-selection policy of its own; the working directory is the
  only root the user already controls, and it is what the test projects and the
  README already tell users to rely on. This keeps R20 (no server is hardcoded)
  intact — the value comes from the environment, not from any server name.
- **Do not add a CLI flag.** A flag would add surface for a value the working
  directory already expresses; `cd` into the project is the documented workflow.

# Acceptance criteria

- The `initialize` request carries `workspaceFolders` (one entry: the working
  directory, with its final path segment as the name) and a matching `rootUri`.
- The client capabilities advertise `workspace.workspaceFolders`.
- When the working directory cannot be resolved, neither field is sent.
- `tests/language_servers.rs` drives `java-lsp` against `test-projects/java` and
  `definition`/`references` resolve across the two source files, which only
  happens once a root is advertised.
- All other servers' project-level tests stay green.

# Implementation plan

## Approach

Thread the root through the handshake at the one place the adapter knows it: the
spawn/handshake boundary. `message::initialize_params` gains a `root:
Option<&Path>` parameter and turns it into `root_uri` plus a single
`workspaceFolders` entry (name = the directory's final path segment);
`Client::spawn` passes `std::env::current_dir()`. Keeping the builder pure — it
still only derives `lsp_types` payloads — keeps it unit-testable. The capability
set gains `workspace.workspaceFolders`. A new Java sample project wires `java-lsp`
into the real-server suite, which also exercises the new root behaviour because
`java-lsp` indexes nothing without a root.

## Steps

- [x] **1. Send the root from the builder.** Add the `root: Option<&Path>`
      parameter to `message::initialize_params`, set `root_uri` and
      `workspace_folders` from it, and add `workspace.workspaceFolders` to the
      client capabilities. Unit-test both the with-root and no-root shapes.
- [x] **2. Pass the working directory at spawn.** `Client::spawn` resolves
      `std::env::current_dir()` and passes it to the builder. Update the client
      tests' call sites.
- [x] **3. Add the Java sample project.** `test-projects/java/` — a Maven
      `pom.xml`, a documented `Person` class, and a `Main` that uses it across
      files plus one deliberate parse error — with its own README.
- [x] **4. Wire the real-server test.** Add `java_lsp_serves_the_java_project`
      (gated on `java-lsp` being on `PATH`) and add `java` to
      `test_projects_are_present` in `tests/language_servers.rs`.
- [x] **5. Update the docs.** Rewrite the workspace-root assumption in
      `test-projects/README.md` (and the Java row/commands), note the root in
      `README.md` and `docs/architecture.md`, and record the change in the
      changelog and the docs log.

## Verification

Implemented and verified in the current working tree. `cargo build --release`
finishes cleanly; `cargo test` passes the unit suite (including the two new
`initialize_params` tests) and `cargo test --test language_servers` passes, with
`java_lsp_serves_the_java_project` exercising the cross-file `definition`/
`references` that the advertised root enables. A manual run of `lsp-mcp java-lsp`
from `test-projects/java` confirmed `definition` → `Person.java` and `references`
spanning both files.
