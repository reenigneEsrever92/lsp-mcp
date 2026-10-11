//! The `definition` tool: `textDocument/definition`, shaped for an agent.
//!
//! It validates the shared 1-based [`PositionArgs`], opens the document on
//! demand through [`App`], asks the server for the definition, and returns each
//! location's URI, 1-based line and column, surrounding code context, and the
//! raw LSP payload. A missing `definitionProvider` capability and a position with
//! no definition both produce a clear tool result, not a crash.

use std::path::PathBuf;
use std::sync::Arc;

use lsp_types::{GotoDefinitionResponse, Location};
use rmcp::model::CallToolResult;
use rmcp::ErrorData;
use serde::Serialize;
use serde_json::Value;

use crate::app::App;
use crate::lsp::message::{self, methods};
use crate::lsp::path_to_uri;
use crate::tools::{
    context_for, error_from, error_result, render_locations_text, success_result,
    validate_position, LocationView, PositionArgs,
};

/// The tool's structured result: the locations plus the raw LSP payload.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DefinitionResult {
    locations: Vec<LocationView>,
    raw: Value,
}

/// Run the `definition` tool, turning every expected failure into an
/// `isError` tool result.
pub async fn run(app: Arc<App>, args: PositionArgs) -> Result<CallToolResult, ErrorData> {
    match definition_impl(&app, &args).await {
        Ok(result) => Ok(result),
        Err(result) => Ok(result),
    }
}

/// The tool body. `Err` already carries the tool-level error result to return.
async fn definition_impl(app: &App, args: &PositionArgs) -> Result<CallToolResult, CallToolResult> {
    if !app.supports_definition() {
        return Err(error_result(
            "the LSP server does not advertise definition support (`definitionProvider`)",
        ));
    }

    let path = PathBuf::from(&args.file);
    let text = app
        .open_document(&path)
        .await
        .map_err(|error| error_from(&error))?;
    let position = validate_position(args, &text).map_err(error_result)?;
    let uri = path_to_uri(&path).map_err(|error| error_from(&error))?;

    let params = message::text_document_position_params(&uri, position);
    let params = serde_json::to_value(params)
        .map_err(|error| error_result(format!("failed to encode the request: {error}")))?;
    let raw = app
        .client()
        .request(methods::DEFINITION, Some(params))
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

    let text = render_text(&views);
    let structured = DefinitionResult {
        locations: views,
        raw,
    };
    let structured = serde_json::to_value(&structured)
        .map_err(|error| error_result(format!("failed to encode the result: {error}")))?;
    Ok(success_result(text, structured))
}

/// Decode the `textDocument/definition` result into plain [`Location`]s.
///
/// The response is `Location | Location[] | LocationLink[] | null`; a `null`
/// result (no definition) decodes to an empty vector.
fn decode_locations(raw: &Value) -> Result<Vec<Location>, String> {
    let response: Option<GotoDefinitionResponse> = serde_json::from_value(raw.clone())
        .map_err(|error| format!("unexpected definition response: {error}"))?;
    Ok(match response {
        None => Vec::new(),
        Some(GotoDefinitionResponse::Scalar(location)) => vec![location],
        Some(GotoDefinitionResponse::Array(locations)) => locations,
        Some(GotoDefinitionResponse::Link(links)) => links
            .into_iter()
            .map(|link| Location {
                uri: link.target_uri,
                range: link.target_range,
            })
            .collect(),
    })
}

/// Render the human-readable text block for the located definitions.
fn render_text(views: &[LocationView]) -> String {
    render_locations_text(views, "No definition found at the given position.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{Position, Range, Uri};
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
    fn decodes_a_single_location() {
        let raw = location("file:///tmp/x.rs", 2, 4);
        let locations = decode_locations(&raw).unwrap();
        assert_eq!(locations.len(), 1);
        assert_eq!(locations[0].range.start, Position::new(2, 4));
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
    }

    #[test]
    fn decodes_location_links() {
        let raw = json!([{
            "targetUri": "file:///tmp/target.rs",
            "targetRange": {
                "start": {"line": 5, "character": 1},
                "end": {"line": 5, "character": 9}
            },
            "targetSelectionRange": {
                "start": {"line": 5, "character": 1},
                "end": {"line": 5, "character": 4}
            }
        }]);
        let locations = decode_locations(&raw).unwrap();
        assert_eq!(locations.len(), 1);
        assert_eq!(locations[0].uri.as_str(), "file:///tmp/target.rs");
        assert_eq!(locations[0].range.start, Position::new(5, 1));
    }

    #[test]
    fn a_non_location_result_is_a_clear_error() {
        assert!(decode_locations(&json!({"not": "a location"})).is_err());
    }

    #[test]
    fn renders_a_uri_and_position_for_each_location() {
        let uri: Uri = "file:///tmp/x.rs".parse().unwrap();
        let view = LocationView::from_range(
            uri.as_str().to_string(),
            &Range::new(Position::new(0, 0), Position::new(0, 3)),
            ">    1 | fn main() {}\n".to_string(),
        );
        let text = render_text(std::slice::from_ref(&view));
        assert!(text.contains("file:///tmp/x.rs:1:1"), "got: {text}");
        assert!(text.contains("fn main()"), "got: {text}");
    }

    #[test]
    fn renders_a_no_definition_message_for_an_empty_result() {
        assert!(render_text(&[]).contains("No definition found"));
    }
}
