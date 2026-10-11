//! Apply a `WorkspaceEdit` to files on disk.
//!
//! This is the shared, **MCP-agnostic** write primitive: it depends only on
//! `lsp_types`, [`crate::error`], and `std::fs` — never on `App`, the MCP server,
//! or any tool argument type — so every tool that writes files (the code-action
//! apply tool here, and later rename and format) reuses it unchanged.
//!
//! The documented entry points are:
//!
//! - [`apply`] — apply a whole [`WorkspaceEdit`] (its `document_changes` when
//!   present, otherwise its `changes`), returning the paths touched.
//! - [`apply_text_edits`] — the per-file primitive, exposed because
//!   `textDocument/formatting` returns a bare `TextEdit[]` rather than a
//!   `WorkspaceEdit`, so format can call it directly.
//!
//! Behaviour:
//!
//! - **Both edit shapes are handled.** `document_changes` is preferred when
//!   present (per the protocol) and covers both plain `TextDocumentEdit`s and
//!   the `Operations` shape, which mixes text edits with `CreateFile`,
//!   `RenameFile`, and `DeleteFile` resource operations.
//! - **In-file edits are applied bottom-to-top.** A workspace edit's edits all
//!   address the *initial* document version, so within a file they are sorted by
//!   range in descending order and applied last-to-first, keeping every earlier
//!   range valid as later text lengths change.
//! - **Positions are UTF-16.** LSP positions count UTF-16 code units while Rust
//!   strings are UTF-8, so a `(line, character)` is converted to a byte offset by
//!   a code-unit-aware scan, never by byte or `char` index.
//! - **Resource operations run in order**, because a rename can precede edits
//!   that target the new path.
//! - Every failure — an unreadable file, a URI that is not a file path, or a
//!   range past the end of the document — is a clear [`Error`], never a panic.

use std::path::PathBuf;

use lsp_types::{
    CreateFile, DeleteFile, DocumentChangeOperation, DocumentChanges, OneOf, Position, RenameFile,
    ResourceOp, TextDocumentEdit, TextEdit, Uri, WorkspaceEdit,
};

use crate::error::{Error, Result};
use crate::lsp::uri_to_path;

/// The files a [`WorkspaceEdit`] touched, grouped by the kind of operation.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct AppliedEdit {
    /// Files whose contents were changed by text edits.
    pub files_changed: Vec<PathBuf>,
    /// Files created by a `CreateFile` operation.
    pub files_created: Vec<PathBuf>,
    /// Files renamed by a `RenameFile` operation (both the old and new paths).
    pub files_renamed: Vec<PathBuf>,
    /// Files deleted by a `DeleteFile` operation.
    pub files_deleted: Vec<PathBuf>,
}

impl AppliedEdit {
    /// Every path the edit touched, deduped, for the result text.
    pub fn affected(&self) -> Vec<PathBuf> {
        let mut paths: Vec<PathBuf> = Vec::new();
        for group in [
            &self.files_changed,
            &self.files_created,
            &self.files_renamed,
            &self.files_deleted,
        ] {
            for path in group {
                if !paths.contains(path) {
                    paths.push(path.clone());
                }
            }
        }
        paths
    }

    /// Sort and dedupe each group so the reported lists are deterministic.
    fn finish(&mut self) {
        for group in [
            &mut self.files_changed,
            &mut self.files_created,
            &mut self.files_renamed,
            &mut self.files_deleted,
        ] {
            group.sort();
            group.dedup();
        }
    }
}

/// Apply a [`WorkspaceEdit`] to disk.
///
/// `document_changes` is preferred when present, otherwise `changes` is used.
/// The returned [`AppliedEdit`] reports which files were changed, created,
/// renamed, and deleted.
pub fn apply(edit: &WorkspaceEdit) -> Result<AppliedEdit> {
    let mut applied = AppliedEdit::default();

    if let Some(document_changes) = &edit.document_changes {
        match document_changes {
            DocumentChanges::Edits(edits) => {
                for text_document_edit in edits {
                    let path = apply_text_document_edit(text_document_edit)?;
                    applied.files_changed.push(path);
                }
            }
            DocumentChanges::Operations(operations) => {
                for operation in operations {
                    match operation {
                        DocumentChangeOperation::Op(op) => apply_resource_op(&mut applied, op)?,
                        DocumentChangeOperation::Edit(text_document_edit) => {
                            let path = apply_text_document_edit(text_document_edit)?;
                            applied.files_changed.push(path);
                        }
                    }
                }
            }
        }
        applied.finish();
        return Ok(applied);
    }

    if let Some(changes) = &edit.changes {
        for (uri, edits) in changes {
            let path = apply_text_edits(uri, edits)?;
            applied.files_changed.push(path);
        }
    }

    applied.finish();
    Ok(applied)
}

/// Apply `edits` to the file at `uri`, writing the whole file back.
///
/// A non-UTF-8 or unreadable file, a URI that is not a file path, and an edit
/// range past the end of the document are all clear errors.
pub fn apply_text_edits(uri: &Uri, edits: &[TextEdit]) -> Result<PathBuf> {
    let path = uri_to_path(uri)?;
    let text = std::fs::read_to_string(&path)?;
    let edited = apply_to_text(&text, edits)?;
    std::fs::write(&path, edited)?;
    Ok(path)
}

/// Apply one `TextDocumentEdit`, unwrapping annotated edits, and return its path.
fn apply_text_document_edit(edit: &TextDocumentEdit) -> Result<PathBuf> {
    let edits: Vec<TextEdit> = edit
        .edits
        .iter()
        .map(|edit| match edit {
            OneOf::Left(text_edit) => text_edit.clone(),
            OneOf::Right(annotated) => annotated.text_edit.clone(),
        })
        .collect();
    apply_text_edits(&edit.text_document.uri, &edits)
}

/// Apply a `CreateFile` operation.
fn apply_create(applied: &mut AppliedEdit, create: &CreateFile) -> Result<()> {
    let path = uri_to_path(&create.uri)?;
    let overwrite = create
        .options
        .as_ref()
        .and_then(|options| options.overwrite)
        .unwrap_or(false);
    let ignore_if_exists = create
        .options
        .as_ref()
        .and_then(|options| options.ignore_if_exists)
        .unwrap_or(false);

    // Overwrite wins over ignoreIfExists.
    if path.exists() && !overwrite {
        if ignore_if_exists {
            return Ok(());
        }
        return Err(Error::Protocol(format!(
            "cannot create {}: it already exists",
            path.display()
        )));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, "")?;
    applied.files_created.push(path);
    Ok(())
}

/// Apply a `RenameFile` operation.
fn apply_rename(applied: &mut AppliedEdit, rename: &RenameFile) -> Result<()> {
    let from = uri_to_path(&rename.old_uri)?;
    let to = uri_to_path(&rename.new_uri)?;
    let overwrite = rename
        .options
        .as_ref()
        .and_then(|options| options.overwrite)
        .unwrap_or(false);
    let ignore_if_exists = rename
        .options
        .as_ref()
        .and_then(|options| options.ignore_if_exists)
        .unwrap_or(false);

    if !from.exists() {
        return Err(Error::Protocol(format!(
            "cannot rename {}: it does not exist",
            from.display()
        )));
    }
    // Overwrite wins over ignoreIfExists.
    if to.exists() && !overwrite {
        if ignore_if_exists {
            return Ok(());
        }
        return Err(Error::Protocol(format!(
            "cannot rename to {}: it already exists",
            to.display()
        )));
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(&from, &to)?;
    applied.files_renamed.push(from);
    applied.files_renamed.push(to);
    Ok(())
}

/// Apply a `DeleteFile` operation.
fn apply_delete(applied: &mut AppliedEdit, delete: &DeleteFile) -> Result<()> {
    let path = uri_to_path(&delete.uri)?;
    let recursive = delete
        .options
        .as_ref()
        .and_then(|options| options.recursive)
        .unwrap_or(false);
    let ignore_if_not_exists = delete
        .options
        .as_ref()
        .and_then(|options| options.ignore_if_not_exists)
        .unwrap_or(false);

    if !path.exists() {
        if ignore_if_not_exists {
            return Ok(());
        }
        return Err(Error::Protocol(format!(
            "cannot delete {}: it does not exist",
            path.display()
        )));
    }
    if path.is_dir() {
        if recursive {
            std::fs::remove_dir_all(&path)?;
        } else {
            std::fs::remove_dir(&path)?;
        }
    } else {
        std::fs::remove_file(&path)?;
    }
    applied.files_deleted.push(path);
    Ok(())
}

/// Apply a resource operation, dispatching to its `std::fs` implementation.
fn apply_resource_op(applied: &mut AppliedEdit, op: &ResourceOp) -> Result<()> {
    match op {
        ResourceOp::Create(create) => apply_create(applied, create),
        ResourceOp::Rename(rename) => apply_rename(applied, rename),
        ResourceOp::Delete(delete) => apply_delete(applied, delete),
    }
}

/// Apply `edits` to `text` in memory, last-to-first so ranges stay valid.
fn apply_to_text(text: &str, edits: &[TextEdit]) -> Result<String> {
    let mut resolved: Vec<(usize, usize, &str)> = Vec::with_capacity(edits.len());
    for edit in edits {
        let start = position_to_offset(text, edit.range.start)?;
        let end = position_to_offset(text, edit.range.end)?;
        if start > end {
            return Err(Error::Protocol(format!(
                "edit range starts after it ends ({start} > {end})"
            )));
        }
        resolved.push((start, end, edit.new_text.as_str()));
    }

    // Descending by start, then end, so applying last-to-first keeps every
    // not-yet-applied range valid.
    resolved.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));

    let mut result = text.to_string();
    for (start, end, new_text) in resolved {
        result.replace_range(start..end, new_text);
    }
    Ok(result)
}

/// Convert a UTF-16 LSP [`Position`] to a byte offset in `text`.
fn position_to_offset(text: &str, position: Position) -> Result<usize> {
    let mut line_start = 0usize;
    let mut current_line = 0u32;
    while current_line < position.line {
        match text[line_start..].find('\n') {
            Some(index) => {
                line_start += index + 1;
                current_line += 1;
            }
            None => {
                return Err(Error::Protocol(format!(
                    "line {} is past the end of the document",
                    position.line
                )));
            }
        }
    }

    let line_end = text[line_start..]
        .find('\n')
        .map(|index| line_start + index)
        .unwrap_or(text.len());
    let line_text = &text[line_start..line_end];

    let mut units = 0u32;
    for (byte_offset, ch) in line_text.char_indices() {
        if units == position.character {
            return Ok(line_start + byte_offset);
        }
        if units > position.character {
            break;
        }
        units += ch.len_utf16() as u32;
    }
    if units == position.character {
        return Ok(line_end);
    }
    if units > position.character {
        return Err(Error::Protocol(format!(
            "character {} falls inside a multi-unit character on line {}",
            position.character, position.line
        )));
    }
    Err(Error::Protocol(format!(
        "character {} is past the end of line {}",
        position.character, position.line
    )))
}

#[cfg(test)]
#[allow(clippy::mutable_key_type)] // `Uri` keys carry interior mutability, which is inherent to the LSP protocol.
mod tests {
    use super::*;
    use lsp_types::AnnotatedTextEdit;
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// A unique temp-file path (not yet created).
    fn temp_path(tag: &str) -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("lsp-mcp-we-{tag}-{}-{n}.txt", std::process::id()))
    }

    fn write_file(path: &std::path::Path, text: &str) {
        std::fs::write(path, text).unwrap();
    }

    fn uri(path: &std::path::Path) -> Uri {
        crate::lsp::path_to_uri(path).unwrap()
    }

    fn edit(start: (u32, u32), end: (u32, u32), new_text: &str) -> TextEdit {
        TextEdit::new(
            lsp_types::Range::new(Position::new(start.0, start.1), Position::new(end.0, end.1)),
            new_text.to_string(),
        )
    }

    #[test]
    fn applies_changes_to_a_file_on_disk() {
        let path = temp_path("changes");
        write_file(&path, "fn main() {\n    let x = 1;\n}\n");

        let mut changes = HashMap::new();
        changes.insert(uri(&path), vec![edit((1, 4), (1, 9), "let answer")]);
        let applied = apply(&WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        })
        .unwrap();

        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "fn main() {\n    let answer = 1;\n}\n"
        );
        assert_eq!(applied.files_changed, vec![path.clone()]);
        assert_eq!(applied.affected(), vec![path.clone()]);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn applies_multiple_edits_on_one_line_in_descending_order() {
        let path = temp_path("order");
        write_file(&path, "abc\n");

        // Insert "1" at the start and "2" at column 3. Applying ascending would
        // corrupt the second range; descending is correct.
        let mut changes = HashMap::new();
        changes.insert(
            uri(&path),
            vec![edit((0, 0), (0, 0), "1"), edit((0, 3), (0, 3), "2")],
        );
        apply(&WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        })
        .unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "1abc2\n");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn converts_utf16_positions_to_offsets() {
        let path = temp_path("utf16");
        // The emoji is four bytes but two UTF-16 code units.
        write_file(&path, "a 😀 b\n");

        // Select the emoji: characters 2..4 (UTF-16 units), replace with "X".
        let mut changes = HashMap::new();
        changes.insert(uri(&path), vec![edit((0, 2), (0, 4), "X")]);
        apply(&WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        })
        .unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "a X b\n");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn utf16_offset_inserts_after_a_multibyte_character() {
        let path = temp_path("utf16-insert");
        write_file(&path, "😀b\n");

        // Character 2 is just after the emoji (which occupies units 0 and 1).
        let mut changes = HashMap::new();
        changes.insert(uri(&path), vec![edit((0, 2), (0, 2), "X")]);
        apply(&WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        })
        .unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "😀Xb\n");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn applies_document_changes_as_edits() {
        let path = temp_path("doc-edits");
        write_file(&path, "hello world\n");

        let edit_struct = TextDocumentEdit {
            text_document: lsp_types::OptionalVersionedTextDocumentIdentifier {
                uri: uri(&path),
                version: None,
            },
            edits: vec![OneOf::Left(edit((0, 0), (0, 5), "goodbye"))],
        };
        let applied = apply(&WorkspaceEdit {
            document_changes: Some(DocumentChanges::Edits(vec![edit_struct])),
            ..Default::default()
        })
        .unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "goodbye world\n");
        assert_eq!(applied.files_changed, vec![path.clone()]);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn unwraps_an_annotated_text_edit() {
        let path = temp_path("annotated");
        write_file(&path, "let x = 1;\n");

        let edit_struct = TextDocumentEdit {
            text_document: lsp_types::OptionalVersionedTextDocumentIdentifier {
                uri: uri(&path),
                version: None,
            },
            edits: vec![OneOf::Right(AnnotatedTextEdit {
                text_edit: edit((0, 4), (0, 5), "y"),
                annotation_id: "note".to_string(),
            })],
        };
        apply(&WorkspaceEdit {
            document_changes: Some(DocumentChanges::Edits(vec![edit_struct])),
            ..Default::default()
        })
        .unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "let y = 1;\n");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn applies_document_changes_as_resource_operations() {
        let dir = std::env::temp_dir().join(format!("lsp-mcp-we-ops-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let created = dir.join("created.txt");
        let renamed = dir.join("renamed.txt");

        let operations = vec![
            DocumentChangeOperation::Op(ResourceOp::Create(CreateFile {
                uri: uri(&created),
                options: None,
                annotation_id: None,
            })),
            DocumentChangeOperation::Op(ResourceOp::Rename(RenameFile {
                old_uri: uri(&created),
                new_uri: uri(&renamed),
                options: None,
                annotation_id: None,
            })),
            DocumentChangeOperation::Op(ResourceOp::Delete(DeleteFile {
                uri: uri(&renamed),
                options: None,
            })),
        ];
        let applied = apply(&WorkspaceEdit {
            document_changes: Some(DocumentChanges::Operations(operations)),
            ..Default::default()
        })
        .unwrap();

        assert!(!created.exists());
        assert!(!renamed.exists());
        assert_eq!(applied.files_created, vec![created.clone()]);
        assert_eq!(
            applied.files_renamed,
            vec![created.clone(), renamed.clone()]
        );
        assert_eq!(applied.files_deleted, vec![renamed.clone()]);

        // The parent directory was created for the file.
        assert!(dir.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mixed_document_changes_apply_an_edit_after_operations() {
        let dir = std::env::temp_dir().join(format!("lsp-mcp-we-mixed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("target.txt");

        let operations = vec![
            DocumentChangeOperation::Op(ResourceOp::Create(CreateFile {
                uri: uri(&path),
                options: None,
                annotation_id: None,
            })),
            DocumentChangeOperation::Edit(TextDocumentEdit {
                text_document: lsp_types::OptionalVersionedTextDocumentIdentifier {
                    uri: uri(&path),
                    version: None,
                },
                edits: vec![OneOf::Left(edit((0, 0), (0, 0), "content"))],
            }),
        ];
        let applied = apply(&WorkspaceEdit {
            document_changes: Some(DocumentChanges::Operations(operations)),
            ..Default::default()
        })
        .unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "content");
        assert_eq!(applied.files_created, vec![path.clone()]);
        assert_eq!(applied.files_changed, vec![path.clone()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_file_is_a_clear_error() {
        let path = temp_path("missing");
        let mut changes = HashMap::new();
        changes.insert(uri(&path), vec![edit((0, 0), (0, 1), "x")]);
        assert!(apply(&WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        })
        .is_err());
    }

    #[test]
    fn an_out_of_range_edit_is_a_clear_error() {
        let path = temp_path("out-of-range");
        write_file(&path, "a\n");
        let mut changes = HashMap::new();
        changes.insert(uri(&path), vec![edit((5, 0), (5, 1), "x")]);
        let error = apply(&WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        })
        .unwrap_err();
        assert!(error.to_string().contains("past the end"), "got: {error}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_character_past_the_end_of_a_line_is_a_clear_error() {
        let path = temp_path("past-column");
        write_file(&path, "ab\n");
        let mut changes = HashMap::new();
        changes.insert(uri(&path), vec![edit((0, 9), (0, 9), "x")]);
        let error = apply(&WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        })
        .unwrap_err();
        assert!(error.to_string().contains("past the end"), "got: {error}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_non_file_uri_is_a_clear_error() {
        let uri: Uri = "https://example.com/x.txt".parse().unwrap();
        assert!(apply_text_edits(&uri, &[edit((0, 0), (0, 0), "x")]).is_err());
    }

    #[test]
    fn an_empty_edit_touches_nothing() {
        let applied = apply(&WorkspaceEdit::default()).unwrap();
        assert!(applied.affected().is_empty());
        assert!(applied.files_changed.is_empty());
    }

    #[test]
    fn affected_dedupes_across_groups() {
        let applied = AppliedEdit {
            files_changed: vec![PathBuf::from("/a"), PathBuf::from("/b")],
            files_created: vec![PathBuf::from("/b"), PathBuf::from("/c")],
            files_renamed: vec![],
            files_deleted: vec![PathBuf::from("/a")],
        };
        assert_eq!(
            applied.affected(),
            vec![
                PathBuf::from("/a"),
                PathBuf::from("/b"),
                PathBuf::from("/c")
            ]
        );
    }

    #[test]
    fn deserializes_a_changes_workspace_edit() {
        // A guard that the JSON shape the protocol and fake server use still maps.
        let value = json!({
            "changes": {
                "file:///tmp/x.txt": [
                    {"range": {"start": {"line": 0, "character": 0},
                               "end": {"line": 0, "character": 0}},
                     "newText": "hi"}
                ]
            }
        });
        let edit: WorkspaceEdit = serde_json::from_value(value).unwrap();
        assert!(edit.changes.is_some());
    }
}
