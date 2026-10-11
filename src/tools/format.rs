//! The `format` write tool: whole-file or range formatting.
//!
//! It selects the mode from the arguments: an **absent** `range` formats the
//! whole file through `textDocument/formatting`, gated on
//! `documentFormattingProvider`; a **present** `range` formats that range through
//! `textDocument/rangeFormatting`, gated on `documentRangeFormattingProvider`. It
//! opens the document on demand, validates a range when present, and applies the
//! bare `TextEdit[]` the server returns through
//! [`crate::workspace_edit::apply_text_edits`]. A `null` or empty edit list is a
//! "no changes" success that writes nothing.
//!
//! After a write the file is refreshed so the adapter's and the server's
//! document view matches disk. Every expected failure — a missing capability, an
//! unreadable file, a bad range — is a clear `isError` result.

use std::path::PathBuf;
use std::sync::Arc;

use lsp_types::{Range, TextEdit};
use rmcp::model::CallToolResult;
use rmcp::ErrorData;
use serde::Serialize;
use serde_json::Value;

use crate::app::App;
use crate::lsp::message::{self, methods};
use crate::lsp::path_to_uri;
use crate::tools::{error_from, error_result, success_result, validate_range, FormatArgs};
use crate::workspace_edit;

/// The tool's structured result: the file, the optional range, the edits
/// applied, the files changed, and the raw LSP payload.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FormatResult {
    /// The file that was formatted.
    file: String,
    /// The 1-based range that was formatted, when range formatting was requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    range: Option<RangeView>,
    /// How many edits the server returned (and the tool applied).
    applied_edits: usize,
    /// The files whose contents changed.
    files_changed: Vec<String>,
    /// The raw `textDocument/formatting` (or range) payload.
    raw: Value,
}

/// A 1-based range in the result, mirroring the tool's range argument.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RangeView {
    line: u32,
    column: u32,
    end_line: u32,
    end_column: u32,
}

impl RangeView {
    /// Convert a 0-based protocol range to the 1-based shape the tool reports.
    fn from_range(range: &Range) -> Self {
        Self {
            line: range.start.line + 1,
            column: range.start.character + 1,
            end_line: range.end.line + 1,
            end_column: range.end.character + 1,
        }
    }
}

/// Run the `format` tool, turning every expected failure into an `isError` tool
/// result.
pub async fn run(app: Arc<App>, args: FormatArgs) -> Result<CallToolResult, ErrorData> {
    match format_impl(&app, &args).await {
        Ok(result) => Ok(result),
        Err(result) => Ok(result),
    }
}

/// The tool body. `Err` already carries the tool-level error result to return.
async fn format_impl(app: &App, args: &FormatArgs) -> Result<CallToolResult, CallToolResult> {
    // Gate on the capability that matches the selected mode, before any I/O.
    if args.range.is_some() {
        if !app.supports_document_range_formatting() {
            return Err(error_result(
                "the LSP server does not advertise range formatting support (`documentRangeFormattingProvider`)",
            ));
        }
    } else if !app.supports_document_formatting() {
        return Err(error_result(
            "the LSP server does not advertise formatting support (`documentFormattingProvider`)",
        ));
    }

    let path = PathBuf::from(&args.file);
    let text = app
        .open_document(&path)
        .await
        .map_err(|error| error_from(&error))?;
    let uri = path_to_uri(&path).map_err(|error| error_from(&error))?;

    let (edits, range, raw) = match &args.range {
        Some(requested) => {
            let validated = validate_range(requested, &text).map_err(error_result)?;
            let params = message::document_range_formatting_params(&uri, validated);
            let params = serde_json::to_value(params)
                .map_err(|error| error_result(format!("failed to encode the request: {error}")))?;
            let raw = app
                .client()
                .request(methods::RANGE_FORMATTING, Some(params))
                .await
                .map_err(|error| error_from(&error))?;
            let edits = decode_edits(&raw).map_err(error_result)?;
            (edits, Some(validated), raw)
        }
        None => {
            let params = message::document_formatting_params(&uri);
            let params = serde_json::to_value(params)
                .map_err(|error| error_result(format!("failed to encode the request: {error}")))?;
            let raw = app
                .client()
                .request(methods::FORMATTING, Some(params))
                .await
                .map_err(|error| error_from(&error))?;
            let edits = decode_edits(&raw).map_err(error_result)?;
            (edits, None, raw)
        }
    };

    let file = path.display().to_string();
    let displayed_range = range.as_ref().map(RangeView::from_range);

    // A null or empty edit list means the file (or range) is already formatted.
    if edits.is_empty() {
        let structured = FormatResult {
            file,
            range: displayed_range,
            applied_edits: 0,
            files_changed: Vec::new(),
            raw,
        };
        return finish(structured, false);
    }

    let applied_edits = edits.len();
    let changed =
        workspace_edit::apply_text_edits(&uri, &edits).map_err(|error| error_from(&error))?;
    app.refresh_documents(std::slice::from_ref(&changed)).await;

    let structured = FormatResult {
        file,
        range: displayed_range,
        applied_edits,
        files_changed: vec![changed.display().to_string()],
        raw,
    };
    finish(structured, true)
}

/// Decode a `textDocument/formatting` (or range) response: a bare `TextEdit[]`
/// or `null`. A `null` result decodes to an empty list.
fn decode_edits(raw: &Value) -> Result<Vec<TextEdit>, String> {
    let edits: Option<Vec<TextEdit>> = serde_json::from_value(raw.clone())
        .map_err(|error| format!("unexpected formatting response: {error}"))?;
    Ok(edits.unwrap_or_default())
}

/// Build the structured result and its text block.
fn finish(structured: FormatResult, changed: bool) -> Result<CallToolResult, CallToolResult> {
    let text = render_text(&structured, changed);
    let structured = serde_json::to_value(&structured)
        .map_err(|error| error_result(format!("failed to encode the result: {error}")))?;
    Ok(success_result(text, structured))
}

/// Render the human-readable text block, naming the file and the scope.
fn render_text(result: &FormatResult, changed: bool) -> String {
    if !changed {
        return format!("{} is already formatted; no changes.", result.file);
    }
    match &result.range {
        Some(range) => format!(
            "Formatted the range {}:{}-{}:{} in {} ({} edit(s)).",
            range.line,
            range.column,
            range.end_line,
            range.end_column,
            result.file,
            result.applied_edits
        ),
        None => format!(
            "Formatted the whole file {} ({} edit(s)).",
            result.file, result.applied_edits
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::Position;
    use serde_json::json;

    #[test]
    fn decodes_a_null_response_to_no_edits() {
        assert!(decode_edits(&Value::Null).unwrap().is_empty());
        assert!(decode_edits(&json!([])).unwrap().is_empty());
    }

    #[test]
    fn decodes_a_text_edit_array() {
        let edits = decode_edits(&json!([{
            "range": {
                "start": {"line": 0, "character": 0},
                "end": {"line": 0, "character": 3}
            },
            "newText": "abc"
        }]))
        .unwrap();
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].new_text, "abc");
    }

    #[test]
    fn a_non_edit_response_is_a_clear_error() {
        assert!(decode_edits(&json!({"not": "an array"})).is_err());
    }

    #[test]
    fn range_view_converts_to_1_based() {
        let range = Range::new(Position::new(1, 2), Position::new(3, 4));
        let view = RangeView::from_range(&range);
        assert_eq!(view.line, 2);
        assert_eq!(view.column, 3);
        assert_eq!(view.end_line, 4);
        assert_eq!(view.end_column, 5);
    }

    #[test]
    fn renders_whole_file_and_range_and_no_changes() {
        let whole = FormatResult {
            file: "/tmp/x.rs".to_string(),
            range: None,
            applied_edits: 2,
            files_changed: vec!["/tmp/x.rs".to_string()],
            raw: Value::Null,
        };
        assert!(render_text(&whole, true).contains("Formatted the whole file /tmp/x.rs"));

        let ranged = FormatResult {
            file: "/tmp/x.rs".to_string(),
            range: Some(RangeView {
                line: 2,
                column: 1,
                end_line: 2,
                end_column: 19,
            }),
            applied_edits: 1,
            files_changed: vec!["/tmp/x.rs".to_string()],
            raw: Value::Null,
        };
        assert!(render_text(&ranged, true).contains("Formatted the range 2:1-2:19"));
        assert!(render_text(&ranged, false).contains("already formatted"));
    }
}
