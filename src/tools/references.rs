//! The `references` tool: `textDocument/references`, shaped for an agent.
//!
//! It gates on the server's `referencesProvider` capability, validates the shared
//! 1-based position (composed through [`ReferencesArgs`]), opens the document on
//! demand, asks for the references, and returns each location's URI, 1-based line
//! and column, surrounding code context, and the raw LSP payload.

use std::path::PathBuf;
use std::sync::Arc;

use lsp_types::Location;
use rmcp::model::CallToolResult;
use rmcp::ErrorData;
use serde::Serialize;
use serde_json::Value;

use crate::app::App;
use crate::lsp::message::{self, methods};
use crate::lsp::path_to_uri;
use crate::tools::{
    context_for, error_from, error_result, render_locations_text, success_result,
    validate_position, LocationView, ReferencesArgs,
};

/// The tool's structured result: the locations plus the raw LSP payload.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReferencesResult {
    locations: Vec<LocationView>,
    raw: Value,
}

/// Run the `references` tool, turning every expected failure into an `isError`
/// tool result.
pub async fn run(app: Arc<App>, args: ReferencesArgs) -> Result<CallToolResult, ErrorData> {
    match references_impl(&app, &args).await {
        Ok(result) => Ok(result),
        Err(result) => Ok(result),
    }
}

/// The tool body. `Err` already carries the tool-level error result to return.
async fn references_impl(
    app: &App,
    args: &ReferencesArgs,
) -> Result<CallToolResult, CallToolResult> {
    if !app.supports_references() {
        return Err(error_result(
            "the LSP server does not advertise references support (`referencesProvider`)",
        ));
    }

    let path = PathBuf::from(&args.position.file);
    let text = app
        .open_document(&path)
        .await
        .map_err(|error| error_from(&error))?;
    let position = validate_position(&args.position, &text).map_err(error_result)?;
    let uri = path_to_uri(&path).map_err(|error| error_from(&error))?;

    let include_declaration = args.include_declaration.unwrap_or(true);
    let params = message::reference_params(&uri, position, include_declaration);
    let params = serde_json::to_value(params)
        .map_err(|error| error_result(format!("failed to encode the request: {error}")))?;
    let raw = app
        .client()
        .request(methods::REFERENCES, Some(params))
        .await
        .map_err(|error| error_from(&error))?;

    let locations = decode_locations(&raw).map_err(error_result)?;
    let views: Vec<LocationView> = locations
        .into_iter()
        .map(|location| {
            let context = context_for(&location.uri, &location.range);
            LocationView::from_range(location.uri.as_str().to_string(), &location.range, context)
        })
        .collect();

    let text = render_locations_text(&views, "No references found at the given position.");
    let structured = ReferencesResult {
        locations: views,
        raw,
    };
    let structured = serde_json::to_value(&structured)
        .map_err(|error| error_result(format!("failed to encode the result: {error}")))?;
    Ok(success_result(text, structured))
}

/// Decode the `textDocument/references` result into plain [`Location`]s.
///
/// The response is `Location[] | null`; a `null` result (no references) decodes
/// to an empty vector.
fn decode_locations(raw: &Value) -> Result<Vec<Location>, String> {
    let response: Option<Vec<Location>> = serde_json::from_value(raw.clone())
        .map_err(|error| format!("unexpected references response: {error}"))?;
    Ok(response.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::Position;
    use serde_json::json;

    fn location(uri: &str, line: u32, character: u32) -> Value {
        json!({
            "uri": uri,
            "range": {
                "start": {"line": line, "character": character},
                "end": {"line": line, "character": character + 3}
            }
        })
    }

    #[test]
    fn decodes_a_null_result_to_no_locations() {
        assert!(decode_locations(&Value::Null).unwrap().is_empty());
    }

    #[test]
    fn decodes_an_array_of_locations() {
        let raw = json!([
            location("file:///tmp/a.rs", 0, 0),
            location("file:///tmp/b.rs", 1, 1),
        ]);
        let locations = decode_locations(&raw).unwrap();
        assert_eq!(locations.len(), 2);
        assert_eq!(locations[1].uri.as_str(), "file:///tmp/b.rs");
        assert_eq!(locations[1].range.start, Position::new(1, 1));
    }

    #[test]
    fn a_non_array_result_is_a_clear_error() {
        assert!(decode_locations(&json!({"not": "locations"})).is_err());
    }
}
