//! The `applyCodeAction` write tool: resolve, then apply a chosen code action.
//!
//! It re-lists the actions from the **same** arguments the `codeAction` tool
//! used (so it knows what is available), selects the action at `index`, optionally
//! guards on `title`, resolves the action with `codeAction/resolve` when it has
//! neither `edit` nor `command`, then applies the resulting change: a
//! `WorkspaceEdit` is written to disk through the shared
//! [`crate::workspace_edit`] helper, a `Command` is run through
//! `workspace/executeCommand`, and when both are present the edit is applied
//! first and the command run after.
//!
//! It gates on `codeActionProvider` (via the shared re-list) and additionally on
//! `executeCommandProvider` before running a command. Every failure — an empty
//! list, an out-of-range index, a title that does not match, an action that
//! resolves to neither edit nor command — is a clear `isError` result.

use std::sync::Arc;

use lsp_types::{CodeAction, CodeActionOrCommand, Command};
use rmcp::model::CallToolResult;
use rmcp::ErrorData;
use serde::Serialize;
use serde_json::Value;

use crate::app::App;
use crate::lsp::message::{self, methods};
use crate::tools::code_action::{action_title, list_actions};
use crate::tools::{error_from, error_result, success_result, ApplyCodeActionArgs};
use crate::workspace_edit::{self, AppliedEdit};

/// The tool's structured result: the files touched, the command run, and the raw
/// payload (the resolved action and/or the command result).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ApplyResult {
    /// The applied action's title.
    title: String,
    /// Files whose contents were changed.
    files_changed: Vec<String>,
    /// Files created.
    files_created: Vec<String>,
    /// Files renamed (both the old and the new path).
    files_renamed: Vec<String>,
    /// Files deleted.
    files_deleted: Vec<String>,
    /// The command that was executed, when one was.
    #[serde(skip_serializing_if = "Option::is_none")]
    executed_command: Option<String>,
    /// The resolved action and/or the command result.
    raw: Value,
}

/// Run the `applyCodeAction` tool, turning every expected failure into an
/// `isError` tool result.
pub async fn run(app: Arc<App>, args: ApplyCodeActionArgs) -> Result<CallToolResult, ErrorData> {
    match apply_impl(&app, &args).await {
        Ok(result) => Ok(result),
        Err(result) => Ok(result),
    }
}

/// The tool body. `Err` already carries the tool-level error result to return.
async fn apply_impl(
    app: &App,
    args: &ApplyCodeActionArgs,
) -> Result<CallToolResult, CallToolResult> {
    let actions = list_actions(app, &args.list_args()).await?;
    if actions.is_empty() {
        return Err(error_result(
            "no code actions are available at this position",
        ));
    }

    let index = args.index as usize;
    let Some(selected) = actions.get(index).cloned() else {
        return Err(error_result(format!(
            "no code action at index {} (the list has {} action(s))",
            args.index,
            actions.len()
        )));
    };

    // The title guard surfaces index drift: the list changed between the list
    // and apply calls, so the index now selects a different action.
    let title = action_title(&selected);
    if let Some(expected) = &args.title {
        if expected != &title {
            return Err(error_result(format!(
                "the code action at index {} is `{title}`, not `{expected}`; re-list and retry",
                args.index
            )));
        }
    }

    match selected {
        CodeActionOrCommand::Command(command) => {
            let (executed, command_result) = run_command(app, &command).await?;
            let raw = json_raw(None, command_result);
            let structured = ApplyResult {
                title,
                files_changed: Vec::new(),
                files_created: Vec::new(),
                files_renamed: Vec::new(),
                files_deleted: Vec::new(),
                executed_command: Some(executed),
                raw,
            };
            finish(structured, &AppliedEdit::default())
        }
        CodeActionOrCommand::CodeAction(action) => apply_action(app, title, action).await,
    }
}

/// Resolve if needed, then apply a `CodeAction`'s edit and/or command.
async fn apply_action(
    app: &App,
    title: String,
    action: CodeAction,
) -> Result<CallToolResult, CallToolResult> {
    let action = if action.edit.is_none() && action.command.is_none() {
        resolve_action(app, &action).await?
    } else {
        action
    };

    if action.edit.is_none() && action.command.is_none() {
        return Err(error_result(
            "the action produced no edit or command to apply",
        ));
    }

    // The protocol applies an action's edit before its command.
    let applied = match &action.edit {
        Some(edit) => workspace_edit::apply(edit).map_err(|error| error_from(&error))?,
        None => AppliedEdit::default(),
    };

    let (executed_command, command_result) = match &action.command {
        Some(command) => {
            let (executed, result) = run_command(app, command).await?;
            (Some(executed), result)
        }
        None => (None, Value::Null),
    };

    let action_value = serde_json::to_value(&action)
        .map_err(|error| error_result(format!("failed to encode the result: {error}")))?;
    let raw = json_raw(Some(action_value), command_result);

    let structured = ApplyResult {
        title,
        files_changed: render_paths(&applied.files_changed),
        files_created: render_paths(&applied.files_created),
        files_renamed: render_paths(&applied.files_renamed),
        files_deleted: render_paths(&applied.files_deleted),
        executed_command,
        raw,
    };
    finish(structured, &applied)
}

/// Resolve an otherwise-empty action through `codeAction/resolve`.
///
/// `codeAction/resolve` takes the `CodeAction` object itself, so no wrapper params
/// type is needed; a rejection is reported as a clear error naming the request.
async fn resolve_action(app: &App, action: &CodeAction) -> Result<CodeAction, CallToolResult> {
    let params = serde_json::to_value(action)
        .map_err(|error| error_result(format!("failed to encode the action: {error}")))?;
    let raw = app
        .client()
        .request(methods::CODE_ACTION_RESOLVE, Some(params))
        .await
        .map_err(|error| error_result(format!("codeAction/resolve failed: {error}")))?;
    serde_json::from_value(raw)
        .map_err(|error| error_result(format!("unexpected codeAction/resolve response: {error}")))
}

/// Execute a `Command` through `workspace/executeCommand`, gated on
/// `executeCommandProvider`. Returns the command id and the raw response.
async fn run_command(app: &App, command: &Command) -> Result<(String, Value), CallToolResult> {
    if !app.supports_execute_command() {
        return Err(error_result(
            "the LSP server does not advertise command execution (`executeCommandProvider`)",
        ));
    }
    let arguments = command.arguments.clone().unwrap_or_default();
    let params = message::execute_command_params(command.command.clone(), arguments);
    let params = serde_json::to_value(params)
        .map_err(|error| error_result(format!("failed to encode the request: {error}")))?;
    let raw = app
        .client()
        .request(methods::EXECUTE_COMMAND, Some(params))
        .await
        .map_err(|error| error_from(&error))?;
    Ok((command.command.clone(), raw))
}

/// Assemble the raw payload from the resolved action and the command result.
fn json_raw(action: Option<Value>, command_result: Value) -> Value {
    serde_json::json!({
        "action": action.unwrap_or(Value::Null),
        "commandResult": command_result,
    })
}

/// Render a list of paths as strings for the result.
fn render_paths(paths: &[std::path::PathBuf]) -> Vec<String> {
    paths
        .iter()
        .map(|path| path.display().to_string())
        .collect()
}

/// Build the structured result and its text block.
fn finish(
    structured: ApplyResult,
    applied: &AppliedEdit,
) -> Result<CallToolResult, CallToolResult> {
    let affected = render_paths(&applied.affected());
    let text = render_text(&structured, &affected);
    let structured = serde_json::to_value(&structured)
        .map_err(|error| error_result(format!("failed to encode the result: {error}")))?;
    Ok(success_result(text, structured))
}

/// Render the human-readable text block, stating which files were affected.
fn render_text(result: &ApplyResult, affected: &[String]) -> String {
    let mut text = format!("Applied the code action `{}`.\n", result.title);
    if !affected.is_empty() {
        text.push_str(&format!("Affected files: {}\n", affected.join(", ")));
    }
    if let Some(command) = &result.executed_command {
        text.push_str(&format!("Executed command `{command}`.\n"));
    }
    if affected.is_empty() && result.executed_command.is_none() {
        text.push_str("The action applied no file changes.\n");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn result(raw: Value) -> ApplyResult {
        ApplyResult {
            title: "Fix it".to_string(),
            files_changed: vec!["/tmp/x.rs".to_string()],
            files_created: Vec::new(),
            files_renamed: Vec::new(),
            files_deleted: Vec::new(),
            executed_command: Some("do.thing".to_string()),
            raw,
        }
    }

    #[test]
    fn renders_the_changed_files_and_command() {
        let text = render_text(&result(Value::Null), &["/tmp/x.rs".to_string()]);
        assert!(text.contains("Fix it"), "got: {text}");
        assert!(text.contains("/tmp/x.rs"), "got: {text}");
        assert!(text.contains("do.thing"), "got: {text}");
    }

    #[test]
    fn renders_no_changes_plainly() {
        let mut empty = result(Value::Null);
        empty.files_changed.clear();
        empty.executed_command = None;
        assert!(render_text(&empty, &[]).contains("no file changes"));
    }

    #[test]
    fn raw_carries_the_action_and_command_result() {
        let raw = json_raw(
            Some(serde_json::json!({"title": "Fix"})),
            serde_json::json!(null),
        );
        assert_eq!(raw["action"]["title"], "Fix");
        assert!(raw["commandResult"].is_null());
    }

    #[test]
    fn render_paths_stringifies_paths() {
        assert_eq!(
            render_paths(&[PathBuf::from("/a/b")]),
            vec!["/a/b".to_string()]
        );
    }
}
