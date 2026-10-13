# Backlog

Every change to `lsp-mcp` starts here as a **change request**: a
`type: ChangeRequest` document that states the problem, the proposal, the
decisions behind it, and the acceptance criteria. The [requirements](../../requirements.md)
are the source these are derived from.

Each change request carries a `kind` and a `state`.

## `kind`

| Kind          | Meaning                                       |
| ------------- | --------------------------------------------- |
| `feature`     | Adds new capability.                          |
| `bug`         | Fixes behaviour that is wrong.                |
| `improvement` | Raises the quality of existing behaviour.     |
| `refactor`    | Changes structure without changing behaviour. |

## `state`

| State         | Meaning                                                                |
| ------------- | ---------------------------------------------------------------------- |
| `proposed`    | Captured and agreed, but not yet planned.                              |
| `planned`     | Has an implementation plan and is ready to build.                      |
| `in-progress` | Being implemented.                                                     |
| `done`        | Implemented and shipped; recorded in the [changelog](../changelog.md). |
| `rejected`    | Decided against.                                                       |
| `superseded`  | Replaced by another request.                                           |

## Requests

| Request                                                                      | Kind        | Priority | State    |
| ---------------------------------------------------------------------------- | ----------- | -------- | -------- |
| [Adapter core, CLI, and LSP client transport](adapter-core-lsp-transport.md) | feature     | high     | done     |
| [MCP stdio server and tool surface](mcp-stdio-server.md)                     | feature     | high     | done     |
| [Read/explore tools](read-explore-tools.md)                                  | feature     | high     | done     |
| [Code-action tools](code-action-tools.md)                                    | feature     | medium   | done     |
| [Write tools: rename and format](write-tools-rename-format.md)               | feature     | medium   | done     |
| [Merge pushed diagnostics with pulled ones](merge-pushed-diagnostics.md)     | improvement | medium   | proposed |
| [Advertise the working directory as the workspace root](workspace-root-from-cwd.md) | improvement | high     | done     |
