//! The `hover` tool: `textDocument/hover`, shaped for an agent.
//!
//! It gates on the server's `hoverProvider` capability, validates the shared
//! 1-based [`PositionArgs`], opens the document on demand, asks for the hover,
//! renders the returned `HoverContents` (type, signature, documentation) and the
//! optional range, and returns the structured result with the raw LSP payload.

use std::path::PathBuf;
use std::sync::Arc;

use lsp_types::{Hover, HoverContents, MarkedString, Range};
use rmcp::model::CallToolResult;
use rmcp::ErrorData;
use serde::Serialize;
use serde_json::Value;

use crate::app::App;
use crate::lsp::message::{self, methods};
use crate::lsp::path_to_uri;
use crate::tools::{
    context_for_text, error_from, error_result, success_result, validate_position, LocationView,
    PositionArgs,
};

/// The rendered hover, with the server's optional range in 1-based form.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HoverView {
    /// The rendered contents (type, signature, documentation).
    contents: String,
    /// The range the hover applies to, when the server provided one.
    #[serde(skip_serializing_if = "Option::is_none")]
    range: Option<LocationView>,
}

/// The tool's structured result: the hover, the queried position's context, and
/// the raw LSP payload.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HoverResult {
    uri: String,
    location: LocationView,
    hover: HoverView,
    raw: Value,
}

/// Run the `hover` tool, turning every expected failure into an `isError` result.
pub async fn run(app: Arc<App>, args: PositionArgs) -> Result<CallToolResult, ErrorData> {
    match hover_impl(&app, &args).await {
        Ok(result) => Ok(result),
        Err(result) => Ok(result),
    }
}

/// The tool body. `Err` already carries the tool-level error result to return.
async fn hover_impl(app: &App, args: &PositionArgs) -> Result<CallToolResult, CallToolResult> {
    if !app.supports_hover() {
        return Err(error_result(
            "the LSP server does not advertise hover support (`hoverProvider`)",
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
        .request(methods::HOVER, Some(params))
        .await
        .map_err(|error| error_from(&error))?;

    let hover: Option<Hover> = serde_json::from_value(raw.clone())
        .map_err(|error| error_result(format!("unexpected hover response: {error}")))?;

    let location = LocationView::from_range(
        uri.as_str().to_string(),
        &Range::new(position, position),
        context_for_text(&text, &Range::new(position, position)),
    );

    let Some(hover) = hover else {
        let text = format!(
            "No hover information at {}:{}:{} .",
            uri.as_str(),
            args.line,
            args.column
        );
        let structured = HoverResult {
            uri: uri.as_str().to_string(),
            location,
            hover: HoverView {
                contents: String::new(),
                range: None,
            },
            raw,
        };
        let structured = serde_json::to_value(&structured)
            .map_err(|error| error_result(format!("failed to encode the result: {error}")))?;
        return Ok(success_result(text, structured));
    };

    let contents = render_contents(&hover.contents);
    let range = hover.range.as_ref().map(|range| {
        LocationView::from_range(
            uri.as_str().to_string(),
            range,
            context_for_text(&text, range),
        )
    });

    let text = if contents.is_empty() {
        "No hover information at the given position.".to_string()
    } else {
        contents.clone()
    };
    let structured = HoverResult {
        uri: uri.as_str().to_string(),
        location,
        hover: HoverView { contents, range },
        raw,
    };
    let structured = serde_json::to_value(&structured)
        .map_err(|error| error_result(format!("failed to encode the result: {error}")))?;
    Ok(success_result(text, structured))
}

/// Render [`HoverContents`] to plain text.
///
/// A scalar renders directly; an array joins its marked strings with newlines;
/// a `MarkupContent` yields its value (its `kind` is dropped).
fn render_contents(contents: &HoverContents) -> String {
    match contents {
        HoverContents::Scalar(marked) => render_marked_string(marked),
        HoverContents::Array(items) => items
            .iter()
            .map(render_marked_string)
            .collect::<Vec<_>>()
            .join("\n"),
        HoverContents::Markup(content) => content.value.clone(),
    }
}

/// Render one [`MarkedString`] to text, fencing a language/code pair.
fn render_marked_string(marked: &MarkedString) -> String {
    match marked {
        MarkedString::String(text) => text.clone(),
        MarkedString::LanguageString(language) => {
            format!("```{}\n{}\n```", language.language, language.value)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{MarkupContent, MarkupKind};
    use serde_json::json;

    #[test]
    fn renders_a_scalar_hover() {
        let contents = HoverContents::Scalar(MarkedString::String("u32".to_string()));
        assert_eq!(render_contents(&contents), "u32");
    }

    #[test]
    fn renders_an_array_hover_joined_by_newlines() {
        let contents = HoverContents::Array(vec![
            MarkedString::String("fn main()".to_string()),
            MarkedString::LanguageString(lsp_types::LanguageString {
                language: "rust".to_string(),
                value: "fn main() {}".to_string(),
            }),
        ]);
        let rendered = render_contents(&contents);
        assert!(rendered.contains("fn main()"), "got: {rendered}");
        assert!(
            rendered.contains("```rust\nfn main() {}\n```"),
            "got: {rendered}"
        );
    }

    #[test]
    fn renders_a_markup_hover_as_its_value() {
        let contents = HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: "# Docs".to_string(),
        });
        assert_eq!(render_contents(&contents), "# Docs");
    }

    #[test]
    fn decodes_a_null_hover() {
        let hover: Option<Hover> = serde_json::from_value(Value::Null).unwrap();
        assert!(hover.is_none());
    }

    #[test]
    fn decodes_a_hover_with_a_range() {
        let raw = json!({
            "contents": {"kind": "markdown", "value": "docs"},
            "range": {"start": {"line": 1, "character": 2},
                      "end": {"line": 1, "character": 5}}
        });
        let hover: Option<Hover> = serde_json::from_value(raw).unwrap();
        let hover = hover.unwrap();
        assert_eq!(render_contents(&hover.contents), "docs");
        assert_eq!(hover.range.unwrap().start, lsp_types::Position::new(1, 2));
    }
}
