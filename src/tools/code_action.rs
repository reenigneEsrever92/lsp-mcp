//! The `codeAction` list tool: `textDocument/codeAction`, shaped for an agent.
//!
//! It is a **read** tool — it never writes. It gates on the server's
//! `codeActionProvider` capability, opens the document on demand, validates the
//! shared 1-based position, gathers the diagnostics at that position (unless
//! `include_diagnostics` is `false`) so a server can offer the matching quickfix,
//! sends `textDocument/codeAction`, and returns one [`ActionView`] per action
//! with the `index` the apply tool selects by, plus the raw payload.
//!
//! The fetch-and-decode logic lives in [`list_actions`], which the
//! `applyCodeAction` tool reuses so it can re-list exactly what this tool
//! returned.

use std::path::PathBuf;
use std::sync::Arc;

use lsp_types::{CodeActionKind, CodeActionOrCommand, CodeActionResponse, Diagnostic, Range};
use rmcp::model::CallToolResult;
use rmcp::ErrorData;
use serde::Serialize;
use serde_json::Value;

use crate::app::App;
use crate::lsp::message::{self, methods};
use crate::lsp::path_to_uri;
use crate::tools::diagnostics::file_diagnostics;
use crate::tools::{error_from, error_result, success_result, validate_position, CodeActionArgs};

/// One code action, in agent-friendly form: the `index` to apply it by plus its
/// identity and shape.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ActionView {
    /// The position of the action in the list; the identity `applyCodeAction` uses.
    index: usize,
    /// The action's human-readable title.
    title: String,
    /// The action's kind, when provided.
    #[serde(skip_serializing_if = "Option::is_none")]
    kind: Option<String>,
    /// Whether the server marked the action preferred.
    #[serde(skip_serializing_if = "Option::is_none")]
    is_preferred: Option<bool>,
    /// Why the action is disabled, when the server marked it so.
    #[serde(skip_serializing_if = "Option::is_none")]
    disabled: Option<String>,
    /// Whether applying the action carries a `WorkspaceEdit`.
    has_edit: bool,
    /// Whether applying the action runs a command.
    has_command: bool,
    /// The command identifier, for a bare `Command` action.
    #[serde(skip_serializing_if = "Option::is_none")]
    command: Option<String>,
}

/// The tool's structured result: the actions plus the raw LSP payload.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CodeActionResult {
    actions: Vec<ActionView>,
    raw: Value,
}

/// Run the `codeAction` tool, turning every expected failure into an `isError`
/// tool result.
pub async fn run(app: Arc<App>, args: CodeActionArgs) -> Result<CallToolResult, ErrorData> {
    match code_action_impl(&app, &args).await {
        Ok(result) => Ok(result),
        Err(result) => Ok(result),
    }
}

/// The tool body. `Err` already carries the tool-level error result to return.
async fn code_action_impl(
    app: &App,
    args: &CodeActionArgs,
) -> Result<CallToolResult, CallToolResult> {
    let response = list_actions(app, args).await?;
    let raw = serde_json::to_value(&response)
        .map_err(|error| error_result(format!("failed to encode the result: {error}")))?;

    let views: Vec<ActionView> = response.iter().enumerate().map(to_view).collect();
    let text = render_text(&views);

    let structured = CodeActionResult {
        actions: views,
        raw,
    };
    let structured = serde_json::to_value(&structured)
        .map_err(|error| error_result(format!("failed to encode the result: {error}")))?;
    Ok(success_result(text, structured))
}

/// Fetch and decode the code actions for `args`.
///
/// Gates on `supports_code_actions`, opens the document on demand, validates the
/// position, gathers the diagnostics at the position, sends
/// `textDocument/codeAction`, and returns the decoded response. Shared with the
/// apply tool so it re-lists exactly what this tool returned.
pub(crate) async fn list_actions(
    app: &App,
    args: &CodeActionArgs,
) -> Result<CodeActionResponse, CallToolResult> {
    if !app.supports_code_actions() {
        return Err(error_result(
            "the LSP server does not advertise code action support (`codeActionProvider`)",
        ));
    }

    let path = PathBuf::from(&args.position.file);
    let text = app
        .open_document(&path)
        .await
        .map_err(|error| error_from(&error))?;
    let position = validate_position(&args.position, &text).map_err(error_result)?;
    let uri = path_to_uri(&path).map_err(|error| error_from(&error))?;

    let diagnostics = gather_diagnostics(app, &path, position, args.include_diagnostics).await;
    let only = args.only.as_ref().map(|kinds| {
        kinds
            .iter()
            .map(|kind| CodeActionKind::from(kind.clone()))
            .collect()
    });

    let params =
        message::code_action_params(&uri, Range::new(position, position), diagnostics, only);
    let params = serde_json::to_value(params)
        .map_err(|error| error_result(format!("failed to encode the request: {error}")))?;
    let raw = app
        .client()
        .request(methods::CODE_ACTION, Some(params))
        .await
        .map_err(|error| error_from(&error))?;

    serde_json::from_value(raw)
        .map_err(|error| error_result(format!("unexpected code action response: {error}")))
}

/// Gather the diagnostics at `position`, best-effort.
///
/// When `include` is not `false`, retrieves the file's diagnostics through the
/// shared helper and keeps only those whose range contains `position`. A server
/// that reports none, or an unreadable file, yields an empty list rather than an
/// error, because the diagnostic scoping is a hint.
async fn gather_diagnostics(
    app: &App,
    path: &std::path::Path,
    position: lsp_types::Position,
    include: Option<bool>,
) -> Vec<Diagnostic> {
    if include == Some(false) {
        return Vec::new();
    }
    match file_diagnostics(app, path).await {
        Ok(retrieved) => retrieved
            .diagnostics
            .into_iter()
            .filter(|diagnostic| range_contains(&diagnostic.range, position))
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Whether `range` contains the zero-width `position`.
fn range_contains(range: &Range, position: lsp_types::Position) -> bool {
    position >= range.start && position <= range.end
}

/// The title of a `CodeActionOrCommand`, for the drift guard and the result.
pub(crate) fn action_title(action: &CodeActionOrCommand) -> String {
    match action {
        CodeActionOrCommand::Command(command) => command.title.clone(),
        CodeActionOrCommand::CodeAction(action) => action.title.clone(),
    }
}

/// Build an [`ActionView`] from an action and its list position.
fn to_view((index, action): (usize, &CodeActionOrCommand)) -> ActionView {
    match action {
        CodeActionOrCommand::Command(command) => ActionView {
            index,
            title: command.title.clone(),
            kind: None,
            is_preferred: None,
            disabled: None,
            has_edit: false,
            has_command: true,
            command: Some(command.command.clone()),
        },
        CodeActionOrCommand::CodeAction(action) => ActionView {
            index,
            title: action.title.clone(),
            kind: action.kind.as_ref().map(|kind| kind.as_str().to_string()),
            is_preferred: action.is_preferred,
            disabled: action
                .disabled
                .as_ref()
                .map(|disabled| disabled.reason.clone()),
            has_edit: action.edit.is_some(),
            has_command: action.command.is_some(),
            command: action
                .command
                .as_ref()
                .map(|command| command.command.clone()),
        },
    }
}

/// Render the human-readable text block, one action per line.
fn render_text(views: &[ActionView]) -> String {
    if views.is_empty() {
        return "No code actions are available at this position.".to_string();
    }
    let mut text = String::new();
    for view in views {
        let mut notes: Vec<String> = Vec::new();
        if let Some(kind) = &view.kind {
            notes.push(kind.clone());
        }
        if view.is_preferred == Some(true) {
            notes.push("preferred".to_string());
        }
        if let Some(disabled) = &view.disabled {
            notes.push(format!("disabled: {disabled}"));
        }
        if let Some(command) = &view.command {
            notes.push(format!("command: {command}"));
        }
        if notes.is_empty() {
            text.push_str(&format!("[{}] {}\n", view.index, view.title));
        } else {
            text.push_str(&format!(
                "[{}] {} ({})\n",
                view.index,
                view.title,
                notes.join(", ")
            ));
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{CodeAction, Command, Position};
    use serde_json::json;

    fn fetch(response: Value) -> CodeActionResponse {
        serde_json::from_value(response).unwrap()
    }

    #[test]
    fn decodes_a_mixed_response() {
        let response = fetch(json!([
            {
                "title": "Fix the thing",
                "kind": "quickfix",
                "edit": {"changes": {}},
                "isPreferred": true
            },
            {"title": "Run the command", "command": "do.thing", "arguments": []},
            {"title": "Deferred", "kind": "refactor", "data": {"id": 1}}
        ]));
        let views: Vec<ActionView> = response.iter().enumerate().map(to_view).collect();

        assert_eq!(views.len(), 3);
        assert_eq!(views[0].index, 0);
        assert_eq!(views[0].title, "Fix the thing");
        assert_eq!(views[0].kind.as_deref(), Some("quickfix"));
        assert_eq!(views[0].is_preferred, Some(true));
        assert!(views[0].has_edit);
        assert!(!views[0].has_command);

        // A bare Command.
        assert_eq!(views[1].command.as_deref(), Some("do.thing"));
        assert!(views[1].has_command);
        assert!(!views[1].has_edit);

        // A deferred action: neither edit nor command.
        assert!(!views[2].has_edit);
        assert!(!views[2].has_command);
    }

    #[test]
    fn renders_disabled_reasons() {
        let response = fetch(json!([
            {"title": "Nope", "kind": "quickfix", "disabled": {"reason": "not applicable"}}
        ]));
        let views: Vec<ActionView> = response.iter().enumerate().map(to_view).collect();
        assert_eq!(views[0].disabled.as_deref(), Some("not applicable"));
        let text = render_text(&views);
        assert!(text.contains("[0] Nope"), "got: {text}");
        assert!(text.contains("disabled: not applicable"), "got: {text}");
    }

    #[test]
    fn renders_an_empty_response_with_a_clear_message() {
        assert!(render_text(&[]).contains("No code actions are available"));
    }

    #[test]
    fn action_title_reads_both_shapes() {
        let command = CodeActionOrCommand::Command(Command {
            title: "Cmd".to_string(),
            command: "c".to_string(),
            arguments: None,
        });
        assert_eq!(action_title(&command), "Cmd");
        let action = CodeActionOrCommand::CodeAction(CodeAction {
            title: "Act".to_string(),
            ..Default::default()
        });
        assert_eq!(action_title(&action), "Act");
    }

    #[test]
    fn range_contains_a_zero_width_position_at_its_edges() {
        let range = Range::new(Position::new(1, 4), Position::new(1, 9));
        assert!(range_contains(&range, Position::new(1, 4)));
        assert!(range_contains(&range, Position::new(1, 9)));
        assert!(range_contains(&range, Position::new(1, 6)));
        assert!(!range_contains(&range, Position::new(1, 3)));
        assert!(!range_contains(&range, Position::new(1, 10)));
    }
}
