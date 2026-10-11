---
type: Reference
title: Front matter schema
description: The metadata fields used by documents in this bundle.
tags: [reference, frontmatter, schema]
status: active
---

# Front matter schema

Every document in this bundle that is not an index carries YAML front matter.
This page defines every field used anywhere in the bundle.

## Concept fields

These fields appear on concept documents (the overview, the requirements, the
reference pages, the changelog) and on change requests, where noted.

| Field         | Required | Values                                                                | Meaning                                                                                       |
| ------------- | -------- | --------------------------------------------------------------------- | --------------------------------------------------------------------------------------------- |
| `type`        | **Yes**  | `Overview`, `Requirements`, `Reference`, `Changelog`, `ChangeRequest` | The kind of document. Determines which other fields apply.                                    |
| `title`       | No       | free text                                                             | A human-readable title. Defaults to the first heading.                                        |
| `description` | No       | one line                                                              | A one-line summary of the document.                                                           |
| `tags`        | No       | list of strings                                                       | Topics for grouping and search. Change requests conventionally start with `dev`.              |
| `status`      | No       | `draft`, `active`, `stable`                                           | The maturity of a _concept_ document. Not used on change requests, which use `state` instead. |

## Change-request fields

A document with `type: ChangeRequest` uses these fields in addition to
`title`, `description`, and `tags`, and **does not** use `status`.

| Field      | Required | Values                                                                 | Meaning                                                                            |
| ---------- | -------- | ---------------------------------------------------------------------- | ---------------------------------------------------------------------------------- |
| `type`     | **Yes**  | `ChangeRequest`                                                        | Marks the document as a change request.                                            |
| `kind`     | **Yes**  | `feature`, `bug`, `improvement`, `refactor`                            | The nature of the change.                                                          |
| `state`    | **Yes**  | `proposed`, `planned`, `in-progress`, `done`, `rejected`, `superseded` | Where the request is in its lifecycle.                                             |
| `priority` | No       | `low`, `medium`, `high`                                                | Relative urgency.                                                                  |
| `owner`    | No       | free text (an actor)                                                   | Who is responsible for the request.                                                |
| `verified` | No       | mapping `{ by: <actor>, at: <timestamp> }`                             | Records that a done request was re-checked against the code, and by whom and when. |

## Reserved filenames

`index.md` and `log.md` are reserved: they carry no front matter. An `index.md`
navigates a section; `log.md` records how the bundle evolves. For the meaning of
`kind` and `state` in practice, see the [backlog index](dev/backlog/index.md).
