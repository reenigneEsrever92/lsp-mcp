//! The `diagnostics` tool: pull (`textDocument/diagnostic`) or push
//! (`textDocument/publishDiagnostics`), shaped for an agent.
//!
//! It prefers **pull** when the server advertises `diagnosticProvider`: it sends
//! one `textDocument/diagnostic` request and decodes the report synchronously.
//! Otherwise it reads the **push** cache: it records the URI's current revision,
//! opens the document (a server that publishes on `didOpen` emits a fresh
//! report), awaits a newer revision under a bounded timeout, and falls back to
//! whatever was already cached. When nothing has arrived it says so plainly.
//! Both paths return a [`DiagnosticView`] per diagnostic with the file's URI,
//! 1-based range, surrounding code context, and the raw LSP payload.
//!
//! The file's diagnostics are retrieved through [`file_diagnostics`], a
//! `pub(crate)` helper shared with the code-action tool's diagnostic scoping, so
//! there is one source of truth for the pull/push logic.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use lsp_types::{
    Diagnostic, DocumentDiagnosticReport, DocumentDiagnosticReportResult, NumberOrString, Uri,
};
use rmcp::model::CallToolResult;
use rmcp::ErrorData;
use serde::Serialize;
use serde_json::Value;

use crate::app::App;
use crate::lsp::message::{self, methods};
use crate::lsp::path_to_uri;
use crate::tools::{
    context_for_text, error_from, error_result, success_result, FileArgs, LocationView,
};

/// How long to await a fresh pushed report when the URI has no cached report yet.
const PUSH_WAIT: Duration = Duration::from_secs(2);
/// How long to await a fresher pushed report when one is already cached.
const PUSH_WAIT_CACHED: Duration = Duration::from_millis(300);

/// One diagnostic, in agent-friendly, 1-based form with surrounding code.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticView {
    /// The severity, rendered as a readable name, when the server provided one.
    #[serde(skip_serializing_if = "Option::is_none")]
    severity: Option<String>,
    /// The diagnostic's code, when provided (numbers are stringified).
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<String>,
    /// The diagnostic's source, when provided.
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    /// The human-readable message.
    message: String,
    /// The diagnostic's range, 1-based, with surrounding code context.
    location: LocationView,
}

/// The tool's structured result: the mechanism used, the diagnostics, and the
/// raw LSP payload.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticsResult {
    mechanism: String,
    diagnostics: Vec<DiagnosticView>,
    raw: Value,
}

/// The diagnostics read for one file, with what the tool needs to shape a result.
pub(crate) struct FileDiagnostics {
    /// The document URI the diagnostics belong to.
    pub uri: Uri,
    /// The document text, for rendering code context without a second read.
    pub text: String,
    /// The decoded diagnostics.
    pub diagnostics: Vec<Diagnostic>,
    /// The raw LSP payload.
    pub raw: Value,
    /// A note to render in place of the summary, when there is one (e.g. an
    /// unchanged pull report).
    pub note: Option<String>,
}

/// Retrieve `path`'s diagnostics: pull when the server advertises
/// `diagnosticProvider`, otherwise the push cache. Shared with the code-action
/// tool so both read diagnostics the same way.
pub(crate) async fn file_diagnostics(
    app: &App,
    path: &Path,
) -> Result<FileDiagnostics, CallToolResult> {
    let uri = path_to_uri(path).map_err(|error| error_from(&error))?;
    if app.supports_pull_diagnostics() {
        pull(app, path, &uri).await
    } else {
        push(app, path, &uri).await
    }
}

/// Run the `diagnostics` tool, turning every expected failure into an `isError`
/// tool result.
pub async fn run(app: Arc<App>, args: FileArgs) -> Result<CallToolResult, ErrorData> {
    match diagnostics_impl(&app, &args).await {
        Ok(result) => Ok(result),
        Err(result) => Ok(result),
    }
}

/// The tool body. `Err` already carries the tool-level error result to return.
async fn diagnostics_impl(app: &App, args: &FileArgs) -> Result<CallToolResult, CallToolResult> {
    let path = PathBuf::from(&args.file);
    let retrieved = file_diagnostics(app, &path).await?;
    let mechanism = if app.supports_pull_diagnostics() {
        "pull"
    } else {
        "publish"
    };
    finish(
        mechanism,
        &retrieved.uri,
        &retrieved.text,
        retrieved.diagnostics,
        retrieved.note.as_deref().unwrap_or(""),
        retrieved.raw,
    )
}

/// Read diagnostics by pulling `textDocument/diagnostic`.
async fn pull(app: &App, path: &Path, uri: &Uri) -> Result<FileDiagnostics, CallToolResult> {
    let text = app
        .open_document(path)
        .await
        .map_err(|error| error_from(&error))?;

    let params = message::document_diagnostic_params(uri);
    let params = serde_json::to_value(params)
        .map_err(|error| error_result(format!("failed to encode the request: {error}")))?;
    let raw = app
        .client()
        .request(methods::DOCUMENT_DIAGNOSTIC, Some(params))
        .await
        .map_err(|error| error_from(&error))?;

    let result: DocumentDiagnosticReportResult = serde_json::from_value(raw.clone())
        .map_err(|error| error_result(format!("unexpected diagnostic response: {error}")))?;

    let (diagnostics, note) = decode_pull_report(result);
    Ok(FileDiagnostics {
        uri: uri.clone(),
        text,
        diagnostics,
        raw,
        note,
    })
}

/// Read diagnostics from the push cache, awaiting a fresh report on `didOpen`.
async fn push(app: &App, path: &Path, uri: &Uri) -> Result<FileDiagnostics, CallToolResult> {
    let key = uri.as_str();
    let baseline = app
        .diagnostics()
        .get(key)
        .await
        .map(|entry| entry.revision)
        .unwrap_or(0);
    let wait = if baseline > 0 {
        PUSH_WAIT_CACHED
    } else {
        PUSH_WAIT
    };

    // Opening the document lets a server that publishes on `didOpen` emit a
    // fresh report, and gives us the text to render context against.
    let text = app
        .open_document(path)
        .await
        .map_err(|error| error_from(&error))?;

    let entry = match app.diagnostics().await_newer(key, baseline, wait).await {
        Some(fresh) => Some(fresh),
        None => app.diagnostics().get(key).await,
    };

    let Some(entry) = entry else {
        return Err(error_result(format!(
            "the LSP server has not published diagnostics for {} yet",
            uri.as_str()
        )));
    };

    // Reconstruct the raw publish payload from the cached diagnostics.
    let raw = serde_json::to_value(lsp_types::PublishDiagnosticsParams {
        uri: uri.clone(),
        diagnostics: entry.diagnostics.clone(),
        version: None,
    })
    .map_err(|error| error_result(format!("failed to encode the result: {error}")))?;

    Ok(FileDiagnostics {
        uri: uri.clone(),
        text,
        diagnostics: entry.diagnostics,
        raw,
        note: None,
    })
}

/// Decode a pull report into its diagnostics and an optional note.
///
/// The related-documents partial result is out of scope for this file-scoped
/// tool, and an unchanged report carries no diagnostics.
fn decode_pull_report(result: DocumentDiagnosticReportResult) -> (Vec<Diagnostic>, Option<String>) {
    match result {
        DocumentDiagnosticReportResult::Report(DocumentDiagnosticReport::Full(report)) => {
            (report.full_document_diagnostic_report.items, None)
        }
        DocumentDiagnosticReportResult::Report(DocumentDiagnosticReport::Unchanged(_)) => (
            Vec::new(),
            Some(
                "No change since the previous request (the server returned an unchanged report)."
                    .to_string(),
            ),
        ),
        DocumentDiagnosticReportResult::Partial(_) => (Vec::new(), None),
    }
}

/// Build the structured result from the decoded diagnostics.
fn finish(
    mechanism: &str,
    uri: &Uri,
    text: &str,
    diagnostics: Vec<Diagnostic>,
    message: &str,
    raw: Value,
) -> Result<CallToolResult, CallToolResult> {
    let views: Vec<DiagnosticView> = diagnostics
        .into_iter()
        .map(|diagnostic| to_view(uri, text, diagnostic))
        .collect();

    let rendered = if !message.is_empty() {
        message.to_string()
    } else if views.is_empty() {
        format!("No diagnostics for {}.", uri.as_str())
    } else {
        render_text(&views)
    };

    let structured = DiagnosticsResult {
        mechanism: mechanism.to_string(),
        diagnostics: views,
        raw,
    };
    let structured = serde_json::to_value(&structured)
        .map_err(|error| error_result(format!("failed to encode the result: {error}")))?;
    Ok(success_result(rendered, structured))
}

/// Convert one [`Diagnostic`] into a [`DiagnosticView`].
fn to_view(uri: &Uri, text: &str, diagnostic: Diagnostic) -> DiagnosticView {
    let context = context_for_text(text, &diagnostic.range);
    DiagnosticView {
        severity: diagnostic.severity.map(render_severity),
        code: diagnostic.code.map(render_code),
        source: diagnostic.source,
        message: diagnostic.message,
        location: LocationView::from_range(uri.as_str().to_string(), &diagnostic.range, context),
    }
}

/// Render a diagnostic severity as a readable, lower-case name.
fn render_severity(severity: lsp_types::DiagnosticSeverity) -> String {
    format!("{severity:?}").to_lowercase()
}

/// Render a diagnostic code (number or string) as a string.
fn render_code(code: NumberOrString) -> String {
    match code {
        NumberOrString::Number(number) => number.to_string(),
        NumberOrString::String(string) => string,
    }
}

/// Render the human-readable text block for the diagnostics.
fn render_text(views: &[DiagnosticView]) -> String {
    let mut text = String::new();
    for view in views {
        let severity = view.severity.as_deref().unwrap_or("diagnostic");
        text.push_str(&format!(
            "{}:{} {}: {}\n",
            view.location.uri, view.location.line, severity, view.message
        ));
        text.push_str(&view.location.context);
        text.push('\n');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn decodes_a_full_pull_report() {
        let raw = json!({
            "kind": "full",
            "items": [{
                "range": {"start": {"line": 0, "character": 0},
                          "end": {"line": 0, "character": 1}},
                "severity": 1,
                "code": "E0001",
                "source": "fake",
                "message": "boom"
            }]
        });
        let result: DocumentDiagnosticReportResult = serde_json::from_value(raw).unwrap();
        let (diagnostics, note) = decode_pull_report(result);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].message, "boom");
        assert!(note.is_none());
    }

    #[test]
    fn decodes_an_unchanged_pull_report() {
        let raw = json!({"kind": "unchanged", "resultId": "r1"});
        let result: DocumentDiagnosticReportResult = serde_json::from_value(raw).unwrap();
        let (diagnostics, note) = decode_pull_report(result);
        assert!(diagnostics.is_empty());
        assert!(note.unwrap().contains("unchanged"));
    }

    #[test]
    fn renders_a_severity_name_and_a_numeric_code() {
        assert_eq!(
            render_severity(lsp_types::DiagnosticSeverity::ERROR),
            "error"
        );
        assert_eq!(
            render_severity(lsp_types::DiagnosticSeverity::WARNING),
            "warning"
        );
        assert_eq!(render_code(NumberOrString::Number(42)), "42");
        assert_eq!(render_code(NumberOrString::String("E1".to_string())), "E1");
    }

    #[test]
    fn builds_a_view_with_1_based_coordinates_and_context() {
        let uri: Uri = "file:///tmp/x.rs".parse().unwrap();
        let diagnostic = Diagnostic {
            range: lsp_types::Range::new(
                lsp_types::Position::new(2, 3),
                lsp_types::Position::new(2, 6),
            ),
            severity: Some(lsp_types::DiagnosticSeverity::ERROR),
            code: Some(NumberOrString::Number(7)),
            source: Some("fake".to_string()),
            message: "boom".to_string(),
            ..Default::default()
        };
        let view = to_view(&uri, "a\nb\nc\n", diagnostic);
        assert_eq!(view.location.line, 3);
        assert_eq!(view.location.column, 4);
        assert_eq!(view.severity.as_deref(), Some("error"));
        assert_eq!(view.code.as_deref(), Some("7"));
        assert!(
            view.location.context.contains(">    3 | c"),
            "got: {}",
            view.location.context
        );
    }
}
