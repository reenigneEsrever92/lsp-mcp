# Development

This project is built change by change. Each unit of work starts as a change
request — a `type: ChangeRequest` document in the [backlog](backlog/index.md)
that states the problem, the proposal, the decisions behind it, and the
acceptance criteria. A change request moves through `proposed` → `planned` →
`in-progress` → `done`; shipped changes are recorded in the
[changelog](changelog.md), newest first.

## Sections

- [Architecture](../architecture.md) — the module boundaries and data flow of
  the adapter.
- [Backlog](backlog/index.md) — the change requests that define the work, with
  the meaning of `kind` and `state`.
- [Changelog](changelog.md) — what has shipped.
- [Requirements](../requirements.md) — the requirement analysis the backlog is
  derived from.
