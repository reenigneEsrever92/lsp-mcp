//! The `workspaceSymbol` tool: `workspace/symbol`, shaped for an agent.
//!
//! It gates on the server's `workspaceSymbolProvider` capability, sends the query,
//! decodes **both** response shapes (the flat `SymbolInformation[]` and the
//! richer `WorkspaceSymbol[]`), and returns a [`SymbolView`] per match. A
//! workspace symbol may carry only a URI (no range); its code context is read on
//! demand from that file, best-effort, so an unreadable file yields an empty
//! context rather than an error.

use std::sync::Arc;

use lsp_types::{OneOf, WorkspaceSymbolResponse};
use rmcp::model::CallToolResult;
use rmcp::ErrorData;
use serde::Serialize;
use serde_json::Value;

use crate::app::App;
use crate::lsp::message::{self, methods};
use crate::tools::{
    context_for, error_from, error_result, success_result, SymbolView, WorkspaceSymbolArgs,
};

/// The tool's structured result: the symbols plus the raw LSP payload.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceSymbolResult {
    symbols: Vec<SymbolView>,
    raw: Value,
}

/// Run the `workspaceSymbol` tool, turning every expected failure into an
/// `isError` tool result.
pub async fn run(app: Arc<App>, args: WorkspaceSymbolArgs) -> Result<CallToolResult, ErrorData> {
    match workspace_symbol_impl(&app, &args).await {
        Ok(result) => Ok(result),
        Err(result) => Ok(result),
    }
}

/// The tool body. `Err` already carries the tool-level error result to return.
async fn workspace_symbol_impl(
    app: &App,
    args: &WorkspaceSymbolArgs,
) -> Result<CallToolResult, CallToolResult> {
    if !app.supports_workspace_symbols() {
        return Err(error_result(
            "the LSP server does not advertise workspace symbol support (`workspaceSymbolProvider`)",
        ));
    }

    let params = message::workspace_symbol_params(&args.query);
    let params = serde_json::to_value(params)
        .map_err(|error| error_result(format!("failed to encode the request: {error}")))?;
    let raw = app
        .client()
        .request(methods::WORKSPACE_SYMBOL, Some(params))
        .await
        .map_err(|error| error_from(&error))?;

    let response: Option<WorkspaceSymbolResponse> = serde_json::from_value(raw.clone())
        .map_err(|error| error_result(format!("unexpected workspace symbol response: {error}")))?;

    let symbols = match response {
        None => Vec::new(),
        Some(WorkspaceSymbolResponse::Flat(information)) => information
            .into_iter()
            .map(|symbol| {
                let uri = symbol.location.uri;
                SymbolView::with_location(
                    symbol.name,
                    symbol.kind,
                    None,
                    symbol.container_name,
                    uri.as_str().to_string(),
                    &symbol.location.range,
                    context_for(&uri, &symbol.location.range),
                )
            })
            .collect(),
        Some(WorkspaceSymbolResponse::Nested(nested)) => nested
            .into_iter()
            .map(|symbol| match symbol.location {
                OneOf::Left(location) => SymbolView::with_location(
                    symbol.name,
                    symbol.kind,
                    None,
                    symbol.container_name,
                    location.uri.as_str().to_string(),
                    &location.range,
                    context_for(&location.uri, &location.range),
                ),
                OneOf::Right(workspace_location) => SymbolView::without_location(
                    symbol.name,
                    symbol.kind,
                    None,
                    symbol.container_name,
                    workspace_location.uri.as_str().to_string(),
                ),
            })
            .collect(),
    };

    let text = render_text(&symbols);
    let structured = WorkspaceSymbolResult { symbols, raw };
    let structured = serde_json::to_value(&structured)
        .map_err(|error| error_result(format!("failed to encode the result: {error}")))?;
    Ok(success_result(text, structured))
}

/// Render the human-readable text block for the matched symbols.
fn render_text(symbols: &[SymbolView]) -> String {
    if symbols.is_empty() {
        return "No workspace symbols matched the query.".to_string();
    }
    let mut text = String::new();
    for symbol in symbols {
        match &symbol.location {
            Some(location) => text.push_str(&format!(
                "{} {} ({}:{}:{})\n",
                symbol.kind, symbol.name, symbol.uri, location.line, location.column
            )),
            None => text.push_str(&format!(
                "{} {} ({})\n",
                symbol.kind, symbol.name, symbol.uri
            )),
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn decodes_the_flat_response() {
        let raw = json!([{
            "name": "main",
            "kind": 12,
            "location": {
                "uri": "file:///tmp/x.rs",
                "range": {"start": {"line": 9, "character": 0},
                          "end": {"line": 9, "character": 4}}
            },
            "containerName": "root"
        }]);
        let response: Option<WorkspaceSymbolResponse> = serde_json::from_value(raw).unwrap();
        assert!(matches!(
            response.unwrap(),
            WorkspaceSymbolResponse::Flat(_)
        ));
    }

    #[test]
    fn decodes_the_nested_response_with_a_bare_uri_location() {
        let raw = json!([{
            "name": "main",
            "kind": 12,
            "location": {"uri": "file:///tmp/x.rs"}
        }]);
        let response: Option<WorkspaceSymbolResponse> = serde_json::from_value(raw).unwrap();
        match response.unwrap() {
            WorkspaceSymbolResponse::Nested(symbols) => match &symbols[0].location {
                OneOf::Right(location) => assert_eq!(location.uri.as_str(), "file:///tmp/x.rs"),
                other => panic!("expected a bare workspace location, got {other:?}"),
            },
            other => panic!("expected a nested response, got {other:?}"),
        }
    }

    #[test]
    fn decodes_a_full_location() {
        // A full location satisfies `SymbolInformation`, so the untagged decoder
        // picks the flat form; either way the range is preserved.
        let raw = json!({
            "name": "main",
            "kind": 12,
            "location": {
                "uri": "file:///tmp/x.rs",
                "range": {"start": {"line": 1, "character": 1},
                          "end": {"line": 1, "character": 5}}
            }
        });
        let response: Option<WorkspaceSymbolResponse> =
            serde_json::from_value(json!([raw])).unwrap();
        match response.unwrap() {
            WorkspaceSymbolResponse::Flat(symbols) => {
                assert_eq!(
                    symbols[0].location.range.start,
                    lsp_types::Position::new(1, 1)
                );
            }
            WorkspaceSymbolResponse::Nested(_) => panic!("a full location decodes as flat"),
        }
    }

    #[test]
    fn a_null_response_yields_no_symbols() {
        let response: Option<WorkspaceSymbolResponse> =
            serde_json::from_value(Value::Null).unwrap();
        assert!(response.is_none());
    }

    #[test]
    fn renders_a_no_match_message() {
        assert!(render_text(&[]).contains("No workspace symbols"));
    }
}
