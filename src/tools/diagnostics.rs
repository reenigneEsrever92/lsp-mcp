//! The `diagnostics` tool: the union of pull (`textDocument/diagnostic`) and
//! push (`textDocument/publishDiagnostics`), shaped for an agent.
//!
//! When the server advertises `diagnosticProvider` it **merges** the two
//! channels: it sends one `textDocument/diagnostic` request and decodes the
//! report synchronously, and it also reads the **push** cache — recording the
//! URI's current revision, opening the document (a server that publishes on
//! `didOpen` emits a fresh report), awaiting a newer revision under a bounded
//! timeout, and falling back to whatever was already cached — then returns the
//! union, de-duplicated. Servers such as rust-analyzer answer pull with only
//! their native diagnostics while delivering `cargo check`/clippy output solely
//! through push, so choosing one channel would under-report. When the server does
//! not advertise pull, it reads the push cache alone and says so plainly when
//! nothing has arrived. Both paths return a [`DiagnosticView`] per diagnostic with
//! the file's URI, 1-based range, surrounding code context, and the raw LSP
//! payload, and name the channels that contributed through the `mechanism` field.
//!
//! The file's diagnostics are retrieved through [`file_diagnostics`], a
//! `pub(crate)` helper shared with the code-action tool's diagnostic scoping, so
//! there is one source of truth for the pull/push logic.

use std::collections::HashSet;
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
    /// The decoded diagnostics, merged across the channels that contributed.
    pub diagnostics: Vec<Diagnostic>,
    /// The raw LSP payload.
    pub raw: Value,
    /// A note to render in place of the summary, when there is one (e.g. an
    /// unchanged pull report).
    pub note: Option<String>,
    /// The channels whose diagnostics are present in the result: `"pull"`,
    /// `"publish"`, or `"pull+publish"`.
    pub mechanism: String,
}

/// Retrieve `path`'s diagnostics: when the server advertises `diagnosticProvider`,
/// the union of the pull report and the push cache; otherwise the push cache
/// alone. Shared with the code-action tool so both read diagnostics the same way.
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
    finish(
        &retrieved.mechanism,
        &retrieved.uri,
        &retrieved.text,
        retrieved.diagnostics,
        retrieved.note.as_deref().unwrap_or(""),
        retrieved.raw,
    )
}

/// Read diagnostics by pulling `textDocument/diagnostic` and merging in any
/// pushed report, de-duplicated.
async fn pull(app: &App, path: &Path, uri: &Uri) -> Result<FileDiagnostics, CallToolResult> {
    let key = uri.as_str();
    // Capture the push baseline before opening the document, so a report
    // published in response to the `didOpen` counts as fresh.
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

    // Opening the document validates the pull request and lets a server that
    // publishes on `didOpen` emit a fresh report, and gives us the text to render
    // context against.
    let text = app
        .open_document(path)
        .await
        .map_err(|error| error_from(&error))?;

    // Pull — the synchronous backbone.
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
    let (pulled, note) = decode_pull_report(result);

    // Push — read the cache, awaiting a report newer than the baseline exactly as
    // the push-only path does, and fall back to whatever is already cached. A
    // server that never pushes leaves this empty.
    let pushed = match app.diagnostics().await_newer(key, baseline, wait).await {
        Some(fresh) => fresh.diagnostics,
        None => app
            .diagnostics()
            .get(key)
            .await
            .map(|entry| entry.diagnostics)
            .unwrap_or_default(),
    };

    let pulled_present = !pulled.is_empty();
    let (diagnostics, pushed_present) = merge_diagnostics(pulled, pushed);
    let mechanism = merged_mechanism(pulled_present, pushed_present).to_string();
    // An unchanged-report note would otherwise replace the rendered summary;
    // keep it only when the merged result is empty.
    let note = if diagnostics.is_empty() { note } else { None };

    Ok(FileDiagnostics {
        uri: uri.clone(),
        text,
        diagnostics,
        raw,
        note,
        mechanism,
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
        mechanism: "publish".to_string(),
    })
}

/// Merge pulled and pushed diagnostics into one de-duplicated list.
///
/// The pulled set comes first; a pushed diagnostic is kept only when it is not
/// already present (or already seen among the pushed ones). Returns the merged
/// list and whether any pushed diagnostic was retained (i.e. the push channel
/// contributed to the result).
fn merge_diagnostics(pulled: Vec<Diagnostic>, pushed: Vec<Diagnostic>) -> (Vec<Diagnostic>, bool) {
    let mut seen: HashSet<DiagnosticKey> = HashSet::new();
    let mut merged: Vec<Diagnostic> = Vec::new();
    for diagnostic in pulled {
        if seen.insert(DiagnosticKey::of(&diagnostic)) {
            merged.push(diagnostic);
        }
    }
    let mut pushed_present = false;
    for diagnostic in pushed {
        if seen.insert(DiagnosticKey::of(&diagnostic)) {
            merged.push(diagnostic);
            pushed_present = true;
        }
    }
    (merged, pushed_present)
}

/// The `mechanism` value for a merged pull result: which channels' diagnostics are
/// present in it.
fn merged_mechanism(pulled_present: bool, pushed_present: bool) -> &'static str {
    match (pulled_present, pushed_present) {
        (true, true) => "pull+publish",
        (_, true) => "publish",
        _ => "pull",
    }
}

/// A diagnostic's identity for de-duplication: range, severity, code, source, and
/// message. Two diagnostics with the same key are the same report.
#[derive(PartialEq, Eq, Hash)]
struct DiagnosticKey {
    start_line: u32,
    start_character: u32,
    end_line: u32,
    end_character: u32,
    severity: Option<String>,
    code: Option<String>,
    source: Option<String>,
    message: String,
}

impl DiagnosticKey {
    fn of(diagnostic: &Diagnostic) -> Self {
        Self {
            start_line: diagnostic.range.start.line,
            start_character: diagnostic.range.start.character,
            end_line: diagnostic.range.end.line,
            end_character: diagnostic.range.end.character,
            severity: diagnostic.severity.map(render_severity),
            code: diagnostic.code.clone().map(render_code),
            source: diagnostic.source.clone(),
            message: diagnostic.message.clone(),
        }
    }
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

    fn diagnostic(
        message: &str,
        line: u32,
        severity: lsp_types::DiagnosticSeverity,
        source: &str,
    ) -> Diagnostic {
        Diagnostic {
            range: lsp_types::Range::new(
                lsp_types::Position::new(line, 0),
                lsp_types::Position::new(line, 3),
            ),
            severity: Some(severity),
            code: Some(NumberOrString::String("E0001".to_string())),
            source: Some(source.to_string()),
            message: message.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn dedupes_a_diagnostic_present_on_both_channels() {
        let native =
            |message: &str| diagnostic(message, 1, lsp_types::DiagnosticSeverity::ERROR, "fake");
        let (merged, pushed_present) = merge_diagnostics(
            vec![native("unused variable")],
            vec![
                native("unused variable"),
                diagnostic(
                    "mutable key type",
                    0,
                    lsp_types::DiagnosticSeverity::WARNING,
                    "clippy",
                ),
            ],
        );

        assert_eq!(merged.len(), 2, "the native report must not be duplicated");
        assert!(pushed_present, "the push-only report must contribute");
        let messages: Vec<&str> = merged.iter().map(|d| d.message.as_str()).collect();
        assert_eq!(messages, vec!["unused variable", "mutable key type"]);
    }

    #[test]
    fn keeps_a_push_only_diagnostic_when_pull_is_empty() {
        let (merged, pushed_present) = merge_diagnostics(
            Vec::new(),
            vec![diagnostic(
                "mutable key type",
                0,
                lsp_types::DiagnosticSeverity::WARNING,
                "clippy",
            )],
        );
        assert_eq!(merged.len(), 1);
        assert!(pushed_present);
    }

    #[test]
    fn reports_no_push_contribution_when_it_only_repeats_pull() {
        let native = || {
            diagnostic(
                "unused variable",
                1,
                lsp_types::DiagnosticSeverity::ERROR,
                "fake",
            )
        };
        let (merged, pushed_present) = merge_diagnostics(vec![native()], vec![native()]);
        assert_eq!(merged.len(), 1);
        assert!(
            !pushed_present,
            "a push report that only repeats pull does not contribute"
        );
    }

    #[test]
    fn keeps_distinct_reports_that_share_a_message() {
        let (merged, pushed_present) = merge_diagnostics(
            vec![diagnostic(
                "same message",
                1,
                lsp_types::DiagnosticSeverity::ERROR,
                "fake",
            )],
            vec![diagnostic(
                "same message",
                2,
                lsp_types::DiagnosticSeverity::ERROR,
                "fake",
            )],
        );
        assert_eq!(merged.len(), 2, "ranges differ, so both are kept");
        assert!(pushed_present);
    }

    #[test]
    fn names_the_contributing_channels() {
        assert_eq!(merged_mechanism(true, false), "pull");
        assert_eq!(merged_mechanism(false, true), "publish");
        assert_eq!(merged_mechanism(true, true), "pull+publish");
        assert_eq!(merged_mechanism(false, false), "pull");
    }
}
