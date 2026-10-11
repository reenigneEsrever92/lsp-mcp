//! The `rename` write tool: prepare, then rename, then apply.
//!
//! It gates on `renameProvider`, opens the document on demand, validates the
//! shared 1-based position, sends `textDocument/prepareRename` to confirm the
//! position names a renameable symbol and to obtain the current name, then sends
//! `textDocument/rename` with the new name. The returned `WorkspaceEdit` is
//! written to disk through the shared [`crate::workspace_edit`] helper
//! **unchanged**, and the affected files are refreshed so the adapter's and the
//! server's document view matches disk. It reports the files changed.
//!
//! Per the request's decision the tool **always** calls `prepareRename` first, so
//! a server that supports rename but not prepareRename returns a clear error
//! naming `textDocument/prepareRename` rather than making an unvalidated write.
//! Every expected failure — a missing capability, a non-renameable position, a
//! `null` edit — is a clear `isError` result and writes nothing.

use std::path::PathBuf;
use std::sync::Arc;

use lsp_types::{Position, PrepareRenameResponse, Range, WorkspaceEdit};
use rmcp::model::CallToolResult;
use rmcp::ErrorData;
use serde::Serialize;
use serde_json::Value;

use crate::app::App;
use crate::lsp::message::{self, methods};
use crate::lsp::path_to_uri;
use crate::tools::{error_from, error_result, success_result, validate_position, RenameArgs};
use crate::workspace_edit;

/// The tool's structured result: the old and new names, the files touched, and
/// the raw prepare/rename payloads.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RenameResult {
    /// The symbol's current name, when the server supplied it.
    #[serde(skip_serializing_if = "Option::is_none")]
    old_name: Option<String>,
    /// The requested new name.
    new_name: String,
    /// Files whose contents were changed.
    files_changed: Vec<String>,
    /// Files created.
    files_created: Vec<String>,
    /// Files renamed (both the old and the new path).
    files_renamed: Vec<String>,
    /// Files deleted.
    files_deleted: Vec<String>,
    /// The raw `prepareRename` and `rename` payloads.
    raw: Value,
}

/// Run the `rename` tool, turning every expected failure into an `isError` tool
/// result.
pub async fn run(app: Arc<App>, args: RenameArgs) -> Result<CallToolResult, ErrorData> {
    match rename_impl(&app, &args).await {
        Ok(result) => Ok(result),
        Err(result) => Ok(result),
    }
}

/// The tool body. `Err` already carries the tool-level error result to return.
async fn rename_impl(app: &App, args: &RenameArgs) -> Result<CallToolResult, CallToolResult> {
    if !app.supports_rename() {
        return Err(error_result(
            "the LSP server does not advertise rename support (`renameProvider`)",
        ));
    }

    let path = PathBuf::from(&args.position.file);
    let text = app
        .open_document(&path)
        .await
        .map_err(|error| error_from(&error))?;
    let position = validate_position(&args.position, &text).map_err(error_result)?;
    let uri = path_to_uri(&path).map_err(|error| error_from(&error))?;

    // Validate the position and learn the old name before editing anything.
    let prepare_params = message::prepare_rename_params(&uri, position);
    let prepare_params = serde_json::to_value(prepare_params)
        .map_err(|error| error_result(format!("failed to encode the request: {error}")))?;
    let prepare_raw = app
        .client()
        .request(methods::PREPARE_RENAME, Some(prepare_params))
        .await
        .map_err(|error| error_result(format!("textDocument/prepareRename failed: {error}")))?;

    let prepared: Option<PrepareRenameResponse> = serde_json::from_value(prepare_raw.clone())
        .map_err(|error| error_result(format!("unexpected prepareRename response: {error}")))?;

    let old_name = match prepared {
        // A `null` result means the position does not name a renameable symbol.
        None => {
            return Err(error_result(
                "the position does not name a renameable symbol",
            ));
        }
        Some(PrepareRenameResponse::Range(range)) => Some(text_over_range(&text, &range)),
        Some(PrepareRenameResponse::RangeWithPlaceholder { placeholder, .. }) => Some(placeholder),
        // The server applies its default range; the old name is unknown.
        Some(PrepareRenameResponse::DefaultBehavior { .. }) => None,
    };

    let rename_params = message::rename_params(&uri, position, args.new_name.clone());
    let rename_params = serde_json::to_value(rename_params)
        .map_err(|error| error_result(format!("failed to encode the request: {error}")))?;
    let raw = app
        .client()
        .request(methods::RENAME, Some(rename_params))
        .await
        .map_err(|error| error_from(&error))?;

    let edit: Option<WorkspaceEdit> = serde_json::from_value(raw.clone())
        .map_err(|error| error_result(format!("unexpected rename response: {error}")))?;
    let Some(edit) = edit else {
        return Err(error_result(
            "the server returned no edits for the rename; nothing was written",
        ));
    };

    let applied = workspace_edit::apply(&edit).map_err(|error| error_from(&error))?;
    app.refresh_documents(&applied.affected()).await;

    let structured = RenameResult {
        old_name,
        new_name: args.new_name.clone(),
        files_changed: render_paths(&applied.files_changed),
        files_created: render_paths(&applied.files_created),
        files_renamed: render_paths(&applied.files_renamed),
        files_deleted: render_paths(&applied.files_deleted),
        raw: serde_json::json!({"prepareRename": prepare_raw, "rename": raw}),
    };
    let affected = render_paths(&applied.affected());
    let text = render_text(&structured, &affected);
    let structured = serde_json::to_value(&structured)
        .map_err(|error| error_result(format!("failed to encode the result: {error}")))?;
    Ok(success_result(text, structured))
}

/// The document text covered by `range`, interpreting its positions as the
/// UTF-16 code-unit offsets the protocol specifies. An out-of-range position
/// yields an empty string rather than a panic.
fn text_over_range(text: &str, range: &Range) -> String {
    match (offset_of(text, range.start), offset_of(text, range.end)) {
        (Some(start), Some(end)) if start <= end => text.get(start..end).unwrap_or("").to_string(),
        _ => String::new(),
    }
}

/// The UTF-8 byte offset of `position` in `text`, treating `character` as a
/// UTF-16 code-unit offset within the line.
fn offset_of(text: &str, position: Position) -> Option<usize> {
    let mut offset = 0usize;
    let mut line = 0u32;
    for current in text.split_inclusive('\n') {
        if line == position.line {
            return Some(offset + utf16_to_byte(current, position.character));
        }
        offset += current.len();
        line += 1;
    }
    // A position just past the final newline is still addressable.
    if line == position.line && position.character == 0 {
        return Some(offset);
    }
    None
}

/// The byte index within `line` for a UTF-16 `character` offset; a `character`
/// past the line's characters clamps to the line's end.
fn utf16_to_byte(line: &str, character: u32) -> usize {
    let mut units = 0u32;
    for (byte_index, ch) in line.char_indices() {
        if units >= character {
            return byte_index;
        }
        units += ch.len_utf16() as u32;
    }
    line.len()
}

/// Render a list of paths as strings for the result.
fn render_paths(paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .map(|path| path.display().to_string())
        .collect()
}

/// Render the human-readable text block, stating which files changed.
fn render_text(result: &RenameResult, affected: &[String]) -> String {
    let mut text = match &result.old_name {
        Some(old) => format!("Renamed `{old}` to `{}`.\n", result.new_name),
        None => format!("Renamed the symbol to `{}`.\n", result.new_name),
    };
    if affected.is_empty() {
        text.push_str("The rename applied no file changes.\n");
    } else {
        text.push_str(&format!("Affected files: {}\n", affected.join(", ")));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn decodes_the_range_variant_and_reads_the_old_name() {
        let response: Option<PrepareRenameResponse> = serde_json::from_value(json!({
            "start": {"line": 1, "character": 8},
            "end": {"line": 1, "character": 14}
        }))
        .unwrap();
        let range = match response {
            Some(PrepareRenameResponse::Range(range)) => range,
            other => panic!("expected a range, got {other:?}"),
        };
        assert_eq!(
            text_over_range("fn main() {\n    let answer = 42;\n}\n", &range),
            "answer"
        );
    }

    #[test]
    fn decodes_the_placeholder_variant() {
        let response: Option<PrepareRenameResponse> = serde_json::from_value(json!({
            "range": {
                "start": {"line": 1, "character": 8},
                "end": {"line": 1, "character": 14}
            },
            "placeholder": "answer"
        }))
        .unwrap();
        match response {
            Some(PrepareRenameResponse::RangeWithPlaceholder { placeholder, .. }) => {
                assert_eq!(placeholder, "answer")
            }
            other => panic!("expected a placeholder, got {other:?}"),
        }
    }

    #[test]
    fn decodes_the_default_behavior_variant() {
        let response: Option<PrepareRenameResponse> =
            serde_json::from_value(json!({"defaultBehavior": true})).unwrap();
        assert!(matches!(
            response,
            Some(PrepareRenameResponse::DefaultBehavior { .. })
        ));
    }

    #[test]
    fn a_null_response_is_not_renameable() {
        let response: Option<PrepareRenameResponse> = serde_json::from_value(json!(null)).unwrap();
        assert!(response.is_none());
    }

    #[test]
    fn text_over_range_handles_an_out_of_range_position() {
        let range = Range::new(Position::new(9, 0), Position::new(9, 1));
        assert_eq!(text_over_range("one\ntwo\n", &range), "");
    }

    #[test]
    fn renders_the_old_and_new_names_and_the_affected_files() {
        let result = RenameResult {
            old_name: Some("old".to_string()),
            new_name: "new".to_string(),
            files_changed: vec!["/tmp/x.rs".to_string()],
            files_created: Vec::new(),
            files_renamed: Vec::new(),
            files_deleted: Vec::new(),
            raw: Value::Null,
        };
        let text = render_text(&result, &["/tmp/x.rs".to_string()]);
        assert!(text.contains("Renamed `old` to `new`"), "got: {text}");
        assert!(text.contains("/tmp/x.rs"), "got: {text}");
    }

    #[test]
    fn renders_an_unknown_old_name_plainly() {
        let result = RenameResult {
            old_name: None,
            new_name: "new".to_string(),
            files_changed: Vec::new(),
            files_created: Vec::new(),
            files_renamed: Vec::new(),
            files_deleted: Vec::new(),
            raw: Value::Null,
        };
        let text = render_text(&result, &[]);
        assert!(text.contains("Renamed the symbol to `new`"), "got: {text}");
        assert!(text.contains("no file changes"), "got: {text}");
    }
}
