---
type: ChangeRequest
kind: improvement
title: Merge pushed diagnostics with pulled ones
description: Return the union of pulled and pushed diagnostics so check/clippy lints are not dropped.
state: done
priority: medium
tags: [dev, improvement, diagnostics]
owner: maintainer
verified: { by: maintainer, at: 2026-10-11T17:35:56Z }
---

# Problem

The `diagnostics` tool reads exactly one LSP channel: when the server advertises
`diagnosticProvider` it calls `textDocument/diagnostic` and never consults the
push cache (`src/tools/diagnostics.rs`). Servers that split diagnostics across
the two channels are therefore under-reported. rust-analyzer is the motivating
case: pull returns only its native diagnostics, while `cargo check`/clippy
diagnostics are delivered solely through `textDocument/publishDiagnostics`.
Against a real rust-analyzer a file carrying warnings such as
`clippy::mutable_key_type` reports "No diagnostics" through the tool even though
the editor shows them. The cost is an agent that concludes a file is clean when
it is not.

# Proposal

Merge the channels instead of choosing between them. When pull is supported,
request it and also read the push cache — awaiting a report published in
response to the `didOpen`, exactly as the existing push path already does — then
return the union of the two, de-duplicated. When pull is not supported, keep
today's push-only behaviour. The tool's contract ("a file's diagnostics") then
holds regardless of how a server distributes diagnostics, while the synchronous,
capability-gated shape is preserved and the result keeps naming which channels
contributed through its `mechanism` field.

# Decisions

- **Kind is `improvement`, not `bug`** — "prefer pull, fall back to push" was a
  deliberate, documented decision (see [Read/explore tools](read-explore-tools.md));
  this makes that mechanism complete rather than correcting behaviour that was
  wrong. Agreed with the maintainer.
- **Approach A — merge pull ∪ push with de-duplication** — chosen over "prefer
  push" because push is not guaranteed to be a superset for every server. A union
  is correct whether the server pushes the complete set (rust-analyzer: push =
  native ∪ check) or only extra diagnostics, and it keeps pull's synchronous
  result as the backbone.
- **De-duplication key: range + severity + code + source + message** —
  rust-analyzer includes its native diagnostics on both channels, so a plain
  concatenation would double them; keying on a diagnostic's identifying fields
  collapses the duplicates while preserving genuinely distinct reports.
- **`mechanism` reports the contributing channels** — `"pull"`, `"publish"`, or
  `"pull+publish"` when both contributed. This keeps the existing `"pull"` and
  `"publish"` values and adds one, so the field stays truthful about provenance.
- **Triggering a check is out of scope** — merging only surfaces what the server
  has produced; a fresh session may not have run `cargo check`/clippy at all.
  Making the tool induce a check (for example a save) is a separate request.
- **The change lives in the shared `file_diagnostics` helper**, so the
  `codeAction` tool's `include_diagnostics` scoping reads the merged set too:
  one source of truth for the pull/push logic.
- **Docs to update** — [`architecture.md`](../../architecture.md) (the living
  reference) must describe the merge and the `mechanism` values; its sibling
  [Read/explore tools](read-explore-tools.md) is a historical request, so it gets
  a short pointer note rather than a rewrite of its recorded decision, and
  [`log.md`](../../log.md) records the change.
- **The reproduction is kept as the regression test** —
  `tests/mcp.rs::merges_diagnostics_pushed_alongside_pull`, backed by a new
  `pull-and-push` mode in the fake server. It fails today and must pass once
  implemented.

# Acceptance criteria

- Given a server that advertises pull, answers it with a native diagnostic, and
  pushes a check-only diagnostic, `diagnostics` returns both, and the native one
  appears exactly once (`merges_diagnostics_pushed_alongside_pull`).
- A server that advertises pull and pushes nothing returns its pulled
  diagnostics with `mechanism: "pull"` (`lists_tools_and_returns_structured_results`
  still passes).
- A server that does not advertise pull still returns pushed diagnostics with
  `mechanism: "publish"` (`returns_pushed_diagnostics` still passes).
- When both channels contribute, `mechanism` is `"pull+publish"`; when only one
  does it is `"pull"` or `"publish"` accordingly.
- `cargo build`, `cargo test`, `cargo clippy`, and `cargo fmt` are clean.
- [`architecture.md`](../../architecture.md) describes the merge and the
  `mechanism` values, and [Read/explore tools](read-explore-tools.md) points to
  this request.

# Implementation plan

## Approach

The merge lives in the shared `file_diagnostics` helper in
`src/tools/diagnostics.rs`, so the `codeAction` tool's `include_diagnostics`
scoping reads the merged set too — one source of truth for the pull/push logic.
The helper keeps its capability gate: when the server does not advertise
`diagnosticProvider` it stays on today's push-only path; when it does, the pull
path becomes a _merge_. It captures the push baseline before `open_document`
(so a report published in response to the `didOpen` counts as fresh), issues the
`textDocument/diagnostic` request, reads the push cache with
`DiagnosticsCache::await_newer` under the existing
`PUSH_WAIT`/`PUSH_WAIT_CACHED` timeouts exactly as the push path does (falling
back to the cached report), and returns the union of the two, de-duplicated.

De-duplication keys a diagnostic on its range (four coordinates), render-severity,
render-code, source, and message, keeping the first occurrence with pulled ones
first, so a diagnostic published on both channels collapses to one while
genuinely distinct reports survive.

`FileDiagnostics` gains a `mechanism` field so the helper — not the caller —
reports what actually contributed: `"pull"`, `"publish"`, or `"pull+publish"`.
`diagnostics_impl` stops recomputing it from the capability and reports the
helper's value; the push-only path reports `"publish"`.

Files to touch:

- `src/tools/diagnostics.rs` — the merge and `mechanism` plumbing, plus focused
  unit tests for the merge/de-dup helpers.
- `src/mcp/server.rs` — the `diagnostics` tool description and the `get_info`
  instructions line.
- `docs/architecture.md` — the `diagnostics` paragraph and the module-table row.
- `docs/dev/backlog/read-explore-tools.md` — a pointer note to this request.
- `docs/log.md` — the `## 2026-10-11` bundle-log entry.
- `docs/dev/changelog.md` — the shipped-change bullet (written on close).

## Steps

- [x] **1. Merge in the pull path (`src/tools/diagnostics.rs`).** Capture the push
      baseline before `open_document`, issue the pull request, await a newer pushed
      report (then fall back to the cached one), and merge the two with the de-dup
      key (range + severity + code + source + message). Add a `mechanism` field to
      `FileDiagnostics` set to `"pull"`/`"publish"`/`"pull+publish"` from which
      channels contributed; set `"publish"` on the push-only path and report the
      helper's value from `diagnostics_impl`. _(AC 1–4.)_
- [x] **2. Unit-test the merge/de-dup logic (`src/tools/diagnostics.rs`).** Cover
      de-duplicating a diagnostic present on both channels, keeping a push-only
      diagnostic, the no-duplicate case, distinct reports that share a message but
      differ in range, and the `mechanism` mapping. _(AC 1, 4.)_
- [x] **3. Update the tool surface copy (`src/mcp/server.rs`).** Rewrite the
      `diagnostics` tool description and the `get_info` instructions line to say it
      merges pull and push. _(Doc coverage.)_
- [x] **4. Update the living reference (`docs/architecture.md`).** Rewrite the
      `diagnostics` paragraph to describe the merge and the `mechanism` values, and
      adjust the `src/tools/diagnostics.rs` module-table row. _(AC 6.)_
- [x] **5. Point from the historical request
      (`docs/dev/backlog/read-explore-tools.md`).** Add a short note beside the
      "prefer pull, fall back to push" decision recording that it was later
      completed by this request. _(AC 6.)_
- [x] **6. Verify.** Run `cargo build`, the full `cargo test` (the previously
      failing `merges_diagnostics_pushed_alongside_pull` must pass, and
      `lists_tools_and_returns_structured_results` / `returns_pushed_diagnostics`
      must stay green), `cargo clippy`, and `cargo fmt`. _(AC 5.)_
- [x] **7. Record and close.** Append the `docs/dev/changelog.md` bullet and the
      `docs/log.md` bundle-log entry, then set this request `state: done` with its
      `verified` stamp. _(AC 6; the skill's closing steps.)_
