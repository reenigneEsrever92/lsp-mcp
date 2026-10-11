//! The reusable tool layer.
//!
//! The MCP server ([`crate::mcp`]) holds the shared [`App`](crate::app::App);
//! every `#[tool]` method there stays thin and delegates to a function here, so
//! tool behaviour lives in one place and later tools reuse the same conversion,
//! validation, result, and code-context helpers.
//!
//! Position-based tools share [`PositionArgs`], whose `line` and `column` are
//! **1-based** (what a person sees in an editor); [`validate_position`] checks
//! them against the document text and converts to the 0-based
//! [`lsp_types::Position`] the protocol uses.

use lsp_types::{Position, Range, SymbolKind, Uri};
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Error;
use crate::lsp::uri_to_path;

pub mod apply_code_action;
pub mod code_action;
pub mod definition;
pub mod diagnostics;
pub mod document_symbol;
pub mod format;
pub mod hover;
pub mod references;
pub mod rename;
pub mod workspace_symbol;

/// How many lines of context to show on each side of a target range.
pub const CONTEXT_RADIUS: u32 = 2;

/// The shared argument shape for position-based tools: a file and a **1-based**
/// line and column.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct PositionArgs {
    /// The file to inspect.
    #[schemars(description = "Absolute path to the file to inspect")]
    pub file: String,
    /// The 1-based line of the position.
    #[schemars(description = "1-based line number of the position (the first line is 1)")]
    pub line: u32,
    /// The 1-based column of the position.
    #[schemars(description = "1-based column number of the position (the first column is 1)")]
    pub column: u32,
}

/// The argument shape for the `references` tool: the shared 1-based position
/// plus whether to include the symbol's declaration in the results.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ReferencesArgs {
    /// The shared file + 1-based line/column contract.
    #[serde(flatten)]
    pub position: PositionArgs,
    /// Whether the declaration itself is included alongside the references.
    #[schemars(
        description = "Include the declaration of the symbol as one of the references (defaults to true)"
    )]
    pub include_declaration: Option<bool>,
}

/// The argument shape for the `workspaceSymbol` tool: a workspace-wide query.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct WorkspaceSymbolArgs {
    /// The symbol query string.
    #[schemars(description = "The query to match workspace symbols against (empty matches all)")]
    pub query: String,
}

/// The argument shape for file-only tools (`documentSymbol`, `diagnostics`): a
/// single file path.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct FileArgs {
    /// The file to inspect.
    #[schemars(description = "Absolute path to the file to inspect")]
    pub file: String,
}

/// The argument shape for the `codeAction` tool: the shared 1-based position
/// plus optional diagnostic scoping and kind filtering.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct CodeActionArgs {
    /// The shared file + 1-based line/column contract.
    #[serde(flatten)]
    pub position: PositionArgs,
    /// Whether the diagnostics at the position are included in the request.
    #[schemars(
        description = "Include the diagnostics at the position in the request (defaults to true); many servers only offer a quickfix when handed the diagnostic it resolves"
    )]
    pub include_diagnostics: Option<bool>,
    /// The code-action kinds to request, when narrowing.
    #[schemars(
        description = "Request only these code-action kinds (e.g. `quickfix`, `refactor`); omit to request all"
    )]
    pub only: Option<Vec<String>>,
}

/// The argument shape for the `applyCodeAction` tool: the same list arguments
/// (so the list is reproduced exactly) plus the chosen action's `index` and an
/// optional `title` drift guard.
///
/// The list fields are declared directly rather than flattened through
/// [`CodeActionArgs`] so the generated schema stays flat.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ApplyCodeActionArgs {
    /// The file to act on.
    #[schemars(description = "Absolute path to the file to act on")]
    pub file: String,
    /// The 1-based line of the position.
    #[schemars(description = "1-based line number of the position (the first line is 1)")]
    pub line: u32,
    /// The 1-based column of the position.
    #[schemars(description = "1-based column number of the position (the first column is 1)")]
    pub column: u32,
    /// The index of the chosen action, as returned by `codeAction`.
    #[schemars(
        description = "The `index` of the action to apply, as returned by the `codeAction` tool"
    )]
    pub index: u32,
    /// An optional guard: the chosen action's title, checked before applying.
    #[schemars(
        description = "The chosen action's `title`; when supplied and it does not match the action at `index`, the apply is refused instead of applying the wrong action"
    )]
    pub title: Option<String>,
    /// Whether the diagnostics at the position are included when re-listing.
    #[schemars(
        description = "Include the diagnostics at the position when re-listing, matching the `codeAction` call (defaults to true)"
    )]
    pub include_diagnostics: Option<bool>,
    /// The code-action kinds to request when re-listing.
    #[schemars(
        description = "Request only these code-action kinds when re-listing, matching the `codeAction` call; omit to request all"
    )]
    pub only: Option<Vec<String>>,
}

impl ApplyCodeActionArgs {
    /// The list arguments this apply call re-fetches with.
    pub fn list_args(&self) -> CodeActionArgs {
        CodeActionArgs {
            position: PositionArgs {
                file: self.file.clone(),
                line: self.line,
                column: self.column,
            },
            include_diagnostics: self.include_diagnostics,
            only: self.only.clone(),
        }
    }
}

/// The argument shape for the `rename` tool: the shared 1-based position plus the
/// new name the symbol should take.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct RenameArgs {
    /// The shared file + 1-based line/column contract.
    #[serde(flatten)]
    pub position: PositionArgs,
    /// The new name for the symbol.
    #[schemars(description = "The new name to rename the symbol to")]
    pub new_name: String,
}

/// The argument shape for the `format` tool: the file plus an optional 1-based
/// range. A **present** `range` selects range formatting
/// (`textDocument/rangeFormatting`); an **absent** one formats the whole file
/// (`textDocument/formatting`).
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct FormatArgs {
    /// The file to format.
    #[schemars(description = "Absolute path to the file to format")]
    pub file: String,
    /// The range to format, when formatting a range rather than the whole file.
    #[schemars(
        description = "The 1-based range to format (line/column/end_line/end_column); omit to format the whole file"
    )]
    pub range: Option<FormatRange>,
}

/// A **1-based** range for the `format` tool.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct FormatRange {
    /// The 1-based line of the range start.
    #[schemars(description = "1-based line number of the range start (the first line is 1)")]
    pub line: u32,
    /// The 1-based column of the range start.
    #[schemars(description = "1-based column number of the range start (the first column is 1)")]
    pub column: u32,
    /// The 1-based line of the range end.
    #[schemars(description = "1-based line number of the range end (the first line is 1)")]
    pub end_line: u32,
    /// The 1-based column of the range end.
    #[schemars(description = "1-based column number of the range end (the first column is 1)")]
    pub end_column: u32,
}

/// A document location in agent-friendly, 1-based form, with surrounding code.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocationView {
    /// The document URI.
    pub uri: String,
    /// The 1-based line of the range start.
    pub line: u32,
    /// The 1-based column of the range start.
    pub column: u32,
    /// The 1-based line of the range end.
    pub end_line: u32,
    /// The 1-based column of the range end.
    pub end_column: u32,
    /// The surrounding code, with line numbers and the target range marked.
    pub context: String,
}

impl LocationView {
    /// Build a view from a 0-based LSP range.
    pub fn from_range(uri: String, range: &Range, context: String) -> Self {
        Self {
            uri,
            line: range.start.line + 1,
            column: range.start.character + 1,
            end_line: range.end.line + 1,
            end_column: range.end.character + 1,
            context,
        }
    }
}

/// One symbol, in agent-friendly form, shared by the `documentSymbol` and
/// `workspaceSymbol` tools.
///
/// `uri` is always present; `location` is present when the server reported a
/// range (a workspace symbol may be a bare URI with no range), and carries the
/// 1-based position and surrounding code read from that file.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SymbolView {
    /// The symbol's name.
    pub name: String,
    /// The symbol's kind, rendered as a readable name (e.g. `function`).
    pub kind: String,
    /// The symbol's detail, when the server provided one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// The name of the symbol that contains this one, when provided.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container_name: Option<String>,
    /// The document URI, always present.
    pub uri: String,
    /// The symbol's location with context, when the server reported a range.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<LocationView>,
}

impl SymbolView {
    /// Build a symbol view whose location carries a 1-based range and context.
    pub fn with_location(
        name: String,
        kind: SymbolKind,
        detail: Option<String>,
        container_name: Option<String>,
        uri: String,
        range: &Range,
        context: String,
    ) -> Self {
        let location = LocationView::from_range(uri.clone(), range, context);
        Self {
            name,
            kind: render_symbol_kind(kind),
            detail,
            container_name,
            uri,
            location: Some(location),
        }
    }

    /// Build a symbol view for a symbol known only by its URI (no range/context).
    pub fn without_location(
        name: String,
        kind: SymbolKind,
        detail: Option<String>,
        container_name: Option<String>,
        uri: String,
    ) -> Self {
        Self {
            name,
            kind: render_symbol_kind(kind),
            detail,
            container_name,
            uri,
            location: None,
        }
    }
}

/// Render an [`SymbolKind`] as a readable, lower-case name (e.g. `function`).
///
/// `lsp-types` renders known kinds in PascalCase via `Debug`; unknown kinds fall
/// back to their numeric debug form, which is passed through.
pub fn render_symbol_kind(kind: SymbolKind) -> String {
    format!("{kind:?}").to_lowercase()
}

/// Validate the 1-based `args` against the document `text` and convert them to
/// the 0-based [`Position`] the protocol uses.
///
/// Rejects an empty path, a line or column of `0`, and a position past the end
/// of the document, each with a message an agent can act on.
pub fn validate_position(args: &PositionArgs, text: &str) -> Result<Position, String> {
    if args.file.trim().is_empty() {
        return Err("`file` must not be empty".to_string());
    }
    position_at(args.line, args.column, text)
}

/// Validate a 1-based `line`/`column` against the document `text` and convert it
/// to the 0-based [`Position`] the protocol uses.
///
/// Rejects a line or column of `0` and a position past the end of the document,
/// each with a message an agent can act on.
pub fn position_at(line: u32, column: u32, text: &str) -> Result<Position, String> {
    if line == 0 {
        return Err("`line` is 1-based, so line 0 is invalid".to_string());
    }
    if column == 0 {
        return Err("`column` is 1-based, so column 0 is invalid".to_string());
    }

    let lines = split_lines(text);
    let line_count = lines.len() as u32;
    if line > line_count {
        return Err(format!(
            "`line` {line} is past the end of the document ({line_count} lines)"
        ));
    }

    let line_text = lines.get((line - 1) as usize).copied().unwrap_or("");
    let columns = line_text.chars().count() as u32 + 1;
    if column > columns {
        return Err(format!(
            "`column` {column} is past the end of line {line} ({} columns)",
            columns - 1
        ));
    }

    Ok(Position::new(line - 1, column - 1))
}

/// Validate a 1-based [`FormatRange`] against the document `text`, converting
/// both ends to the 0-based [`Range`] the protocol uses and rejecting an end
/// before the start.
///
/// Each end is validated through [`position_at`], so an out-of-range line or
/// column is a clear error.
pub fn validate_range(range: &FormatRange, text: &str) -> Result<Range, String> {
    let start = position_at(range.line, range.column, text)?;
    let end = position_at(range.end_line, range.end_column, text)?;
    if end < start {
        return Err("the range end is before its start".to_string());
    }
    Ok(Range::new(start, end))
}

/// Render the lines around a target range, with line numbers and each line in
/// the range marked with `>`.
pub fn render_context(text: &str, line: u32, end_line: u32, radius: u32) -> String {
    let lines = split_lines(text);
    let total = lines.len() as u32;
    let start = line.saturating_sub(radius).max(1);
    let end = end_line.saturating_add(radius).min(total);

    let mut rendered = String::new();
    let mut current = start;
    while current <= end {
        let marker = if current >= line && current <= end_line {
            '>'
        } else {
            ' '
        };
        let content = lines.get((current - 1) as usize).copied().unwrap_or("");
        rendered.push_str(&format!("{marker} {current:>4} | {content}\n"));
        current += 1;
    }
    rendered
}

/// Render the code context for `range` by reading the document at `uri` from
/// disk. Returns an empty string when the file cannot be read.
pub fn context_for(uri: &Uri, range: &Range) -> String {
    let Ok(path) = uri_to_path(uri) else {
        return String::new();
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return String::new();
    };
    context_for_text(&text, range)
}

/// Render the code context for `range` from an already-read document `text`.
pub fn context_for_text(text: &str, range: &Range) -> String {
    render_context(
        text,
        range.start.line + 1,
        range.end.line + 1,
        CONTEXT_RADIUS,
    )
}

/// Render a human-readable block for a list of located views, one entry per
/// location prefixed by its URI and 1-based position; `empty` is returned when
/// there are none.
pub fn render_locations_text(views: &[LocationView], empty: &str) -> String {
    if views.is_empty() {
        return empty.to_string();
    }
    let mut text = String::new();
    for view in views {
        text.push_str(&format!("{}:{}:{}\n", view.uri, view.line, view.column));
        text.push_str(&view.context);
        text.push('\n');
    }
    text
}

/// Build a success result carrying both a human-readable text block and the
/// structured content an agent can act on.
pub fn success_result(text: String, structured: Value) -> CallToolResult {
    let mut result = CallToolResult::success(vec![ContentBlock::text(text)]);
    result.structured_content = Some(structured);
    result
}

/// Build a tool-level error result: a result the client renders and the agent
/// can act on, rather than a protocol error a client hides.
pub fn error_result(message: impl Into<String>) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(message.into())])
}

/// Map an adapter failure to a tool-level error result.
pub fn error_from(error: &Error) -> CallToolResult {
    error_result(error.to_string())
}

/// Split `text` into lines, keeping the trailing empty line a final newline
/// implies so an end-of-file position is addressable.
fn split_lines(text: &str) -> Vec<&str> {
    text.split('\n').collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn args(file: &str, line: u32, column: u32) -> PositionArgs {
        PositionArgs {
            file: file.to_string(),
            line,
            column,
        }
    }

    #[test]
    fn converts_a_1_based_position_to_0_based() {
        let position = validate_position(&args("/tmp/x.rs", 3, 4), "one\ntwo\nthree\n").unwrap();
        assert_eq!(position, Position::new(2, 3));
    }

    #[test]
    fn accepts_a_position_at_the_end_of_a_line() {
        // "fn main() {}" has 12 columns; column 13 is the position just past it.
        assert!(validate_position(&args("/tmp/x.rs", 1, 13), "fn main() {}\n").is_ok());
        assert!(validate_position(&args("/tmp/x.rs", 1, 14), "fn main() {}\n").is_err());
    }

    #[test]
    fn rejects_a_line_of_zero() {
        let err = validate_position(&args("/tmp/x.rs", 0, 1), "a\n").unwrap_err();
        assert!(err.contains("1-based"), "got: {err}");
    }

    #[test]
    fn rejects_a_column_of_zero() {
        let err = validate_position(&args("/tmp/x.rs", 1, 0), "a\n").unwrap_err();
        assert!(err.contains("1-based"), "got: {err}");
    }

    #[test]
    fn rejects_a_missing_path() {
        assert!(validate_position(&args("", 1, 1), "a\n").is_err());
        assert!(validate_position(&args("   ", 1, 1), "a\n").is_err());
    }

    #[test]
    fn rejects_a_position_past_the_end_of_the_document() {
        let err = validate_position(&args("/tmp/x.rs", 5, 1), "a\nb\n").unwrap_err();
        assert!(err.contains("past the end"), "got: {err}");
    }

    #[test]
    fn renders_context_with_line_numbers_and_a_target_marker() {
        let text = "one\ntwo\nthree\nfour\nfive\n";
        let rendered = render_context(text, 3, 3, 1);
        assert!(rendered.contains(">    3 | three"), "got: {rendered}");
        assert!(rendered.contains("     2 | two"), "got: {rendered}");
        assert!(rendered.contains("     4 | four"), "got: {rendered}");
        // Radius 1 stops before the second neighbour.
        assert!(!rendered.contains("one"), "got: {rendered}");
        assert!(!rendered.contains("five"), "got: {rendered}");
    }

    #[test]
    fn renders_a_range_across_multiple_lines() {
        let rendered = render_context("a\nb\nc\n", 2, 3, 0);
        assert!(rendered.contains(">    2 | b"));
        assert!(rendered.contains(">    3 | c"));
    }

    #[test]
    fn maps_an_adapter_error_to_a_flagged_tool_result() {
        let error = Error::ServerExited;
        let result = error_from(&error);
        assert_eq!(result.is_error, Some(true));
        let text = result.content[0].as_text().map(|text| text.text.clone());
        assert_eq!(text.as_deref(), Some("LSP server exited unexpectedly"));
    }

    #[test]
    fn builds_a_success_result_with_text_and_structured_content() {
        let result = success_result("hello".to_string(), json!({"locations": []}));
        assert_eq!(result.is_error, Some(false));
        assert_eq!(
            result.content[0].as_text().map(|text| text.text.as_str()),
            Some("hello")
        );
        assert_eq!(result.structured_content, Some(json!({"locations": []})));
    }

    #[test]
    fn references_args_flatten_the_position_and_add_include_declaration() {
        let schema = rmcp::schemars::schema_for!(ReferencesArgs);
        let properties = schema.as_object().unwrap()["properties"]
            .as_object()
            .unwrap();
        assert!(properties.contains_key("file"));
        assert!(properties.contains_key("line"));
        assert!(properties.contains_key("column"));
        assert!(properties.contains_key("include_declaration"));
    }

    #[test]
    fn file_and_workspace_symbol_args_expose_their_field() {
        let file = rmcp::schemars::schema_for!(FileArgs);
        assert!(file.as_object().unwrap()["properties"]
            .as_object()
            .unwrap()
            .contains_key("file"));
        let query = rmcp::schemars::schema_for!(WorkspaceSymbolArgs);
        assert!(query.as_object().unwrap()["properties"]
            .as_object()
            .unwrap()
            .contains_key("query"));
    }

    #[test]
    fn code_action_args_expose_the_position_and_filters() {
        let schema = rmcp::schemars::schema_for!(CodeActionArgs);
        let properties = schema.as_object().unwrap()["properties"]
            .as_object()
            .unwrap();
        for field in ["file", "line", "column", "include_diagnostics", "only"] {
            assert!(properties.contains_key(field), "missing `{field}`");
        }
    }

    #[test]
    fn apply_code_action_args_expose_a_flat_schema() {
        let schema = rmcp::schemars::schema_for!(ApplyCodeActionArgs);
        let properties = schema.as_object().unwrap()["properties"]
            .as_object()
            .unwrap();
        for field in ["file", "line", "column", "index", "title"] {
            assert!(properties.contains_key(field), "missing `{field}`");
        }
    }

    #[test]
    fn apply_code_action_args_build_their_list_arguments() {
        let args = ApplyCodeActionArgs {
            file: "/tmp/x.rs".to_string(),
            line: 3,
            column: 4,
            index: 1,
            title: Some("Fix".to_string()),
            include_diagnostics: Some(false),
            only: Some(vec!["quickfix".to_string()]),
        };
        let list = args.list_args();
        assert_eq!(list.position.file, "/tmp/x.rs");
        assert_eq!(list.position.line, 3);
        assert_eq!(list.position.column, 4);
        assert_eq!(list.include_diagnostics, Some(false));
        assert_eq!(list.only, Some(vec!["quickfix".to_string()]));
    }

    #[test]
    fn rename_args_flatten_the_position_and_add_new_name() {
        let schema = rmcp::schemars::schema_for!(RenameArgs);
        let properties = schema.as_object().unwrap()["properties"]
            .as_object()
            .unwrap();
        for field in ["file", "line", "column", "new_name"] {
            assert!(properties.contains_key(field), "missing `{field}`");
        }
    }

    #[test]
    fn format_args_decode_a_snake_case_range() {
        let args: FormatArgs = serde_json::from_value(json!({
            "file": "/tmp/x.rs",
            "range": {"line": 1, "column": 2, "end_line": 3, "end_column": 4}
        }))
        .unwrap();
        assert_eq!(args.file, "/tmp/x.rs");
        let range = args.range.expect("a range");
        assert_eq!(range.line, 1);
        assert_eq!(range.column, 2);
        assert_eq!(range.end_line, 3);
        assert_eq!(range.end_column, 4);

        let schema = rmcp::schemars::schema_for!(FormatArgs);
        let properties = schema.as_object().unwrap()["properties"]
            .as_object()
            .unwrap();
        assert!(properties.contains_key("file"));
        assert!(properties.contains_key("range"));
    }

    #[test]
    fn validates_a_1_based_range_and_converts_it() {
        let range = FormatRange {
            line: 1,
            column: 1,
            end_line: 2,
            end_column: 6,
        };
        let converted = validate_range(&range, "one\nhello\nworld\n").unwrap();
        assert_eq!(converted.start, Position::new(0, 0));
        assert_eq!(converted.end, Position::new(1, 5));
    }

    #[test]
    fn rejects_a_range_whose_end_precedes_its_start() {
        let range = FormatRange {
            line: 2,
            column: 1,
            end_line: 1,
            end_column: 1,
        };
        let err = validate_range(&range, "one\ntwo\n").unwrap_err();
        assert!(err.contains("before"), "got: {err}");
    }

    #[test]
    fn rejects_a_range_end_past_the_end_of_the_document() {
        let range = FormatRange {
            line: 1,
            column: 1,
            end_line: 9,
            end_column: 1,
        };
        assert!(validate_range(&range, "one\n").is_err());
    }

    #[test]
    fn renders_symbol_kinds_as_readable_lower_case_names() {
        assert_eq!(render_symbol_kind(SymbolKind::FUNCTION), "function");
        assert_eq!(render_symbol_kind(SymbolKind::STRUCT), "struct");
    }

    #[test]
    fn symbol_view_carries_a_location_with_1_based_coordinates() {
        let view = SymbolView::with_location(
            "main".to_string(),
            SymbolKind::FUNCTION,
            Some("fn main()".to_string()),
            None,
            "file:///tmp/x.rs".to_string(),
            &Range::new(Position::new(2, 3), Position::new(2, 6)),
            ">    3 | fn main() {}\n".to_string(),
        );
        let location = view.location.as_ref().unwrap();
        assert_eq!(location.line, 3);
        assert_eq!(location.column, 4);
        assert_eq!(view.kind, "function");
        assert_eq!(view.detail.as_deref(), Some("fn main()"));
    }
}
