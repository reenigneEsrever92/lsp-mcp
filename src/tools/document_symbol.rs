//! The `documentSymbol` tool: `textDocument/documentSymbol`, shaped for an agent.
//!
//! It gates on the server's `documentSymbolProvider` capability, opens the file,
//! asks for its symbols, decodes **both** response shapes (the flat
//! `SymbolInformation[]` and the hierarchical `DocumentSymbol[]`, whose children
//! are flattened), and returns a [`SymbolView`] per symbol with its URI, 1-based
//! selection position, and surrounding code context read from the opened file.

use std::path::PathBuf;
use std::sync::Arc;

use lsp_types::{DocumentSymbol, DocumentSymbolResponse, SymbolInformation};
use rmcp::model::CallToolResult;
use rmcp::ErrorData;
use serde::Serialize;
use serde_json::Value;

use crate::app::App;
use crate::lsp::message::{self, methods};
use crate::lsp::path_to_uri;
use crate::tools::{
    context_for_text, error_from, error_result, success_result, FileArgs, SymbolView,
};

/// The tool's structured result: the symbols plus the raw LSP payload.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DocumentSymbolResult {
    symbols: Vec<SymbolView>,
    raw: Value,
}

/// Run the `documentSymbol` tool, turning every expected failure into an
/// `isError` tool result.
pub async fn run(app: Arc<App>, args: FileArgs) -> Result<CallToolResult, ErrorData> {
    match document_symbol_impl(&app, &args).await {
        Ok(result) => Ok(result),
        Err(result) => Ok(result),
    }
}

/// The tool body. `Err` already carries the tool-level error result to return.
async fn document_symbol_impl(
    app: &App,
    args: &FileArgs,
) -> Result<CallToolResult, CallToolResult> {
    if !app.supports_document_symbols() {
        return Err(error_result(
            "the LSP server does not advertise document symbol support (`documentSymbolProvider`)",
        ));
    }

    let path = PathBuf::from(&args.file);
    let text = app
        .open_document(&path)
        .await
        .map_err(|error| error_from(&error))?;
    let uri = path_to_uri(&path).map_err(|error| error_from(&error))?;

    let params = message::document_symbol_params(&uri);
    let params = serde_json::to_value(params)
        .map_err(|error| error_result(format!("failed to encode the request: {error}")))?;
    let raw = app
        .client()
        .request(methods::DOCUMENT_SYMBOL, Some(params))
        .await
        .map_err(|error| error_from(&error))?;

    let response: Option<DocumentSymbolResponse> = serde_json::from_value(raw.clone())
        .map_err(|error| error_result(format!("unexpected document symbol response: {error}")))?;

    let mut symbols = Vec::new();
    match response {
        None => {}
        Some(DocumentSymbolResponse::Flat(information)) => {
            for symbol in information {
                symbols.push(from_symbol_information(symbol, &text));
            }
        }
        Some(DocumentSymbolResponse::Nested(nested)) => {
            for symbol in nested {
                flatten_document_symbol(symbol, uri.as_str(), &text, &mut symbols);
            }
        }
    }

    let text_block = render_text(&symbols);
    let structured = DocumentSymbolResult { symbols, raw };
    let structured = serde_json::to_value(&structured)
        .map_err(|error| error_result(format!("failed to encode the result: {error}")))?;
    Ok(success_result(text_block, structured))
}

/// Flatten one [`DocumentSymbol`] (and its children) into a [`SymbolView`].
fn flatten_document_symbol(
    symbol: DocumentSymbol,
    uri: &str,
    text: &str,
    out: &mut Vec<SymbolView>,
) {
    let DocumentSymbol {
        name,
        detail,
        kind,
        selection_range,
        children,
        ..
    } = symbol;
    out.push(SymbolView::with_location(
        name,
        kind,
        detail,
        None,
        uri.to_string(),
        &selection_range,
        context_for_text(text, &selection_range),
    ));
    if let Some(children) = children {
        for child in children {
            flatten_document_symbol(child, uri, text, out);
        }
    }
}

/// Build a [`SymbolView`] from a flat [`SymbolInformation`].
fn from_symbol_information(symbol: SymbolInformation, text: &str) -> SymbolView {
    SymbolView::with_location(
        symbol.name,
        symbol.kind,
        None,
        symbol.container_name,
        symbol.location.uri.as_str().to_string(),
        &symbol.location.range,
        context_for_text(text, &symbol.location.range),
    )
}

/// Render the human-readable text block for the listed symbols.
fn render_text(symbols: &[SymbolView]) -> String {
    if symbols.is_empty() {
        return "No symbols found in the file.".to_string();
    }
    let mut text = String::new();
    for symbol in symbols {
        match &symbol.location {
            Some(location) => {
                text.push_str(&format!(
                    "{} {} ({}:{}:{})\n",
                    symbol.kind, symbol.name, symbol.uri, location.line, location.column
                ));
            }
            None => {
                text.push_str(&format!(
                    "{} {} ({})\n",
                    symbol.kind, symbol.name, symbol.uri
                ));
            }
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
                "range": {"start": {"line": 2, "character": 3},
                          "end": {"line": 2, "character": 7}}
            },
            "containerName": "root"
        }]);
        let response: Option<DocumentSymbolResponse> = serde_json::from_value(raw).unwrap();
        match response.unwrap() {
            DocumentSymbolResponse::Flat(items) => {
                let view = from_symbol_information(items.into_iter().next().unwrap(), "a\nb\nc\n");
                assert_eq!(view.name, "main");
                assert_eq!(view.kind, "function");
                assert_eq!(view.container_name.as_deref(), Some("root"));
                assert_eq!(view.location.as_ref().unwrap().line, 3);
            }
            other => panic!("expected a flat response, got {other:?}"),
        }
    }

    #[test]
    fn flattens_the_nested_response_including_children() {
        let raw = json!([{
            "name": "root",
            "kind": 5,
            "range": {"start": {"line": 0, "character": 0},
                      "end": {"line": 4, "character": 1}},
            "selectionRange": {"start": {"line": 0, "character": 0},
                               "end": {"line": 0, "character": 4}},
            "children": [{
                "name": "method",
                "kind": 6,
                "detail": "fn method()",
                "range": {"start": {"line": 1, "character": 0},
                          "end": {"line": 3, "character": 1}},
                "selectionRange": {"start": {"line": 1, "character": 0},
                                   "end": {"line": 1, "character": 6}}
            }]
        }]);
        let response: Option<DocumentSymbolResponse> = serde_json::from_value(raw).unwrap();
        match response.unwrap() {
            DocumentSymbolResponse::Nested(nested) => {
                let mut out = Vec::new();
                for symbol in nested {
                    flatten_document_symbol(
                        symbol,
                        "file:///tmp/x.rs",
                        "a\nb\nc\nd\ne\n",
                        &mut out,
                    );
                }
                assert_eq!(out.len(), 2);
                assert_eq!(out[0].name, "root");
                assert_eq!(out[0].uri, "file:///tmp/x.rs");
                assert_eq!(out[1].name, "method");
                assert_eq!(out[1].detail.as_deref(), Some("fn method()"));
                assert_eq!(out[1].kind, "method");
            }
            other => panic!("expected a nested response, got {other:?}"),
        }
    }

    #[test]
    fn a_null_response_yields_no_symbols() {
        let response: Option<DocumentSymbolResponse> = serde_json::from_value(Value::Null).unwrap();
        assert!(response.is_none());
    }

    #[test]
    fn renders_a_no_symbols_message() {
        assert!(render_text(&[]).contains("No symbols"));
    }
}
