//! The JSON-RPC 2.0 envelope and thin `lsp_types`-derived builders.
//!
//! The adapter hand-rolls the JSON-RPC envelope so it stays server-agnostic and
//! can pass arbitrary or server-specific methods through, while the *payloads*
//! of the standard requests and notifications come from `lsp-types`.

use lsp_types::{
    CodeActionCapabilityResolveSupport, CodeActionClientCapabilities, CodeActionContext,
    CodeActionKind, CodeActionKindLiteralSupport, CodeActionLiteralSupport, CodeActionParams,
    Diagnostic, DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DocumentDiagnosticParams, DocumentFormattingParams, DocumentRangeFormattingParams,
    DocumentSymbolParams, ExecuteCommandParams, FormattingOptions, InitializeParams,
    PartialResultParams, Position, Range, ReferenceContext, ReferenceParams,
    RenameClientCapabilities, RenameParams, ResourceOperationKind, TextDocumentContentChangeEvent,
    TextDocumentIdentifier, TextDocumentItem, TextDocumentPositionParams, Uri,
    VersionedTextDocumentIdentifier, WorkDoneProgressParams, WorkspaceEditClientCapabilities,
    WorkspaceSymbolParams,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The JSON-RPC version string used on every message.
pub const JSONRPC_VERSION: &str = "2.0";

/// The JSON-RPC "method not found" error code.
pub const METHOD_NOT_FOUND: i64 = -32601;

fn default_jsonrpc() -> String {
    JSONRPC_VERSION.to_string()
}

/// A JSON-RPC request or response identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Id {
    /// A numeric id, used by the adapter for its own requests.
    Number(i64),
    /// A string id, accepted when a server picks one.
    String(String),
}

/// A JSON-RPC request (client to server, or server to client).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    /// Always `"2.0"`.
    #[serde(default = "default_jsonrpc")]
    pub jsonrpc: String,
    /// The request id.
    pub id: Id,
    /// The method name.
    pub method: String,
    /// The method parameters, if any.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub params: Option<Value>,
}

/// A JSON-RPC notification (no id, no response expected).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Notification {
    /// Always `"2.0"`.
    #[serde(default = "default_jsonrpc")]
    pub jsonrpc: String,
    /// The method name.
    pub method: String,
    /// The method parameters, if any.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub params: Option<Value>,
}

/// A JSON-RPC response. Exactly one of `result` or `error` should be set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    /// Always `"2.0"`.
    #[serde(default = "default_jsonrpc")]
    pub jsonrpc: String,
    /// The id of the request this responds to.
    pub id: Id,
    /// The success result.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub result: Option<Value>,
    /// The error, when the call failed.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub error: Option<ErrorObject>,
}

/// A JSON-RPC error object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorObject {
    /// The error code.
    pub code: i64,
    /// The human-readable message.
    pub message: String,
    /// Optional additional data.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub data: Option<Value>,
}

/// The LSP method names the adapter uses directly.
pub mod methods {
    /// `initialize`.
    pub const INITIALIZE: &str = "initialize";
    /// `initialized`.
    pub const INITIALIZED: &str = "initialized";
    /// `textDocument/definition`.
    pub const DEFINITION: &str = "textDocument/definition";
    /// `textDocument/references`.
    pub const REFERENCES: &str = "textDocument/references";
    /// `textDocument/hover`.
    pub const HOVER: &str = "textDocument/hover";
    /// `textDocument/documentSymbol`.
    pub const DOCUMENT_SYMBOL: &str = "textDocument/documentSymbol";
    /// `workspace/symbol`.
    pub const WORKSPACE_SYMBOL: &str = "workspace/symbol";
    /// `textDocument/diagnostic` (pull).
    pub const DOCUMENT_DIAGNOSTIC: &str = "textDocument/diagnostic";
    /// `textDocument/codeAction`.
    pub const CODE_ACTION: &str = "textDocument/codeAction";
    /// `codeAction/resolve`.
    pub const CODE_ACTION_RESOLVE: &str = "codeAction/resolve";
    /// `workspace/executeCommand`.
    pub const EXECUTE_COMMAND: &str = "workspace/executeCommand";
    /// `textDocument/prepareRename`.
    pub const PREPARE_RENAME: &str = "textDocument/prepareRename";
    /// `textDocument/rename`.
    pub const RENAME: &str = "textDocument/rename";
    /// `textDocument/formatting`.
    pub const FORMATTING: &str = "textDocument/formatting";
    /// `textDocument/rangeFormatting`.
    pub const RANGE_FORMATTING: &str = "textDocument/rangeFormatting";
    /// `textDocument/publishDiagnostics` (push).
    pub const PUBLISH_DIAGNOSTICS: &str = "textDocument/publishDiagnostics";
    /// `textDocument/didOpen`.
    pub const DID_OPEN: &str = "textDocument/didOpen";
    /// `textDocument/didChange`.
    pub const DID_CHANGE: &str = "textDocument/didChange";
    /// `textDocument/didClose`.
    pub const DID_CLOSE: &str = "textDocument/didClose";
}

/// Build a request with the given id, method, and params.
pub fn request(id: Id, method: impl Into<String>, params: Option<Value>) -> Request {
    Request {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id,
        method: method.into(),
        params,
    }
}

/// Build a notification with the given method and params.
pub fn notification(method: impl Into<String>, params: Option<Value>) -> Notification {
    Notification {
        jsonrpc: JSONRPC_VERSION.to_string(),
        method: method.into(),
        params,
    }
}

/// Build the `initialize` params the adapter sends during the handshake.
#[allow(deprecated)]
pub fn initialize_params(process_id: Option<u32>) -> InitializeParams {
    InitializeParams {
        process_id,
        root_path: None,
        root_uri: None,
        initialization_options: None,
        capabilities: client_capabilities(),
        trace: None,
        workspace_folders: None,
        client_info: Some(lsp_types::ClientInfo {
            name: "lsp-mcp".to_string(),
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
        }),
        locale: None,
        work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
    }
}

/// Build the minimal client capabilities the adapter advertises during the
/// handshake so a real server enables the read/explore features.
///
/// Each feature is advertised as `Some(Default::default())`; a server that gates
/// a feature on the client's declared support (references, hover, document
/// symbols, publish and pull diagnostics, workspace symbols) then enables it.
///
/// The code-action tools additionally advertise `textDocument.codeAction` (with
/// literal, data, and resolve support, so servers return `CodeAction` objects
/// carrying `data` rather than bare `Command`s), `workspace.workspaceEdit` (with
/// `documentChanges` and create/rename/delete resource operations, so a server
/// may send the richer edit shape the workspace-edit helper handles), and
/// `workspace.executeCommand`.
///
/// The write tools advertise `textDocument.rename` (with `prepareSupport`, so a
/// server enables `textDocument/prepareRename`), `textDocument.formatting`, and
/// `textDocument.rangeFormatting`.
fn client_capabilities() -> lsp_types::ClientCapabilities {
    lsp_types::ClientCapabilities {
        workspace: Some(lsp_types::WorkspaceClientCapabilities {
            symbol: Some(Default::default()),
            workspace_edit: Some(WorkspaceEditClientCapabilities {
                document_changes: Some(true),
                resource_operations: Some(vec![
                    ResourceOperationKind::Create,
                    ResourceOperationKind::Rename,
                    ResourceOperationKind::Delete,
                ]),
                ..Default::default()
            }),
            execute_command: Some(Default::default()),
            ..Default::default()
        }),
        text_document: Some(lsp_types::TextDocumentClientCapabilities {
            hover: Some(Default::default()),
            references: Some(Default::default()),
            document_symbol: Some(Default::default()),
            publish_diagnostics: Some(Default::default()),
            diagnostic: Some(Default::default()),
            rename: Some(RenameClientCapabilities {
                prepare_support: Some(true),
                ..Default::default()
            }),
            formatting: Some(Default::default()),
            range_formatting: Some(Default::default()),
            code_action: Some(CodeActionClientCapabilities {
                code_action_literal_support: Some(CodeActionLiteralSupport {
                    code_action_kind: CodeActionKindLiteralSupport {
                        value_set: vec![
                            CodeActionKind::QUICKFIX.as_str().to_string(),
                            CodeActionKind::REFACTOR.as_str().to_string(),
                            CodeActionKind::REFACTOR_EXTRACT.as_str().to_string(),
                            CodeActionKind::REFACTOR_INLINE.as_str().to_string(),
                            CodeActionKind::REFACTOR_REWRITE.as_str().to_string(),
                            CodeActionKind::SOURCE.as_str().to_string(),
                            CodeActionKind::SOURCE_ORGANIZE_IMPORTS.as_str().to_string(),
                            CodeActionKind::SOURCE_FIX_ALL.as_str().to_string(),
                        ],
                    },
                }),
                data_support: Some(true),
                resolve_support: Some(CodeActionCapabilityResolveSupport {
                    properties: vec!["edit".to_string(), "command".to_string()],
                }),
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// Build the params for `textDocument/didOpen`.
pub fn did_open_params(
    uri: &Uri,
    language_id: String,
    version: i32,
    text: String,
) -> DidOpenTextDocumentParams {
    DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            language_id,
            version,
            text,
        },
    }
}

/// Build the params for a full-sync `textDocument/didChange`.
///
/// A change event with no range is defined by the protocol to replace the whole
/// document, so this is valid whether the server advertises full or incremental
/// synchronization.
pub fn did_change_params(uri: &Uri, version: i32, text: String) -> DidChangeTextDocumentParams {
    DidChangeTextDocumentParams {
        text_document: VersionedTextDocumentIdentifier {
            uri: uri.clone(),
            version,
        },
        content_changes: vec![TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text,
        }],
    }
}

/// Build the params for `textDocument/didClose`.
pub fn did_close_params(uri: &Uri) -> DidCloseTextDocumentParams {
    DidCloseTextDocumentParams {
        text_document: TextDocumentIdentifier { uri: uri.clone() },
    }
}

/// Build the params for a request at a position in a document.
///
/// Used by the position-based requests (`textDocument/definition` and the ones
/// later tools add); `position` carries the already-converted **0-based**
/// [`Position`].
pub fn text_document_position_params(uri: &Uri, position: Position) -> TextDocumentPositionParams {
    TextDocumentPositionParams {
        text_document: TextDocumentIdentifier { uri: uri.clone() },
        position,
    }
}

/// Build the params for `textDocument/references`.
///
/// Composes the plain [`TextDocumentPositionParams`] with the reference
/// `context`; `include_declaration` selects whether the declaration itself is
/// listed alongside the references.
pub fn reference_params(
    uri: &Uri,
    position: Position,
    include_declaration: bool,
) -> ReferenceParams {
    ReferenceParams {
        text_document_position: text_document_position_params(uri, position),
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
        context: ReferenceContext {
            include_declaration,
        },
    }
}

/// Build the params for `textDocument/documentSymbol`.
pub fn document_symbol_params(uri: &Uri) -> DocumentSymbolParams {
    DocumentSymbolParams {
        text_document: TextDocumentIdentifier { uri: uri.clone() },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    }
}

/// Build the params for `workspace/symbol`.
pub fn workspace_symbol_params(query: &str) -> WorkspaceSymbolParams {
    WorkspaceSymbolParams {
        partial_result_params: PartialResultParams::default(),
        work_done_progress_params: WorkDoneProgressParams::default(),
        query: query.to_string(),
    }
}

/// Build the params for `textDocument/diagnostic` (pull).
///
/// The adapter always asks for a full report: it sends no identifier and no
/// previous result id, so a server cannot answer `unchanged`.
pub fn document_diagnostic_params(uri: &Uri) -> DocumentDiagnosticParams {
    DocumentDiagnosticParams {
        text_document: TextDocumentIdentifier { uri: uri.clone() },
        identifier: None,
        previous_result_id: None,
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    }
}

/// Build the params for `textDocument/codeAction`.
///
/// `range` is the range the actions apply to, `diagnostics` are the diagnostics
/// at that range (many servers only offer a quickfix when handed the diagnostic
/// it resolves), and `only` optionally narrows the requested action kinds.
pub fn code_action_params(
    uri: &Uri,
    range: Range,
    diagnostics: Vec<Diagnostic>,
    only: Option<Vec<CodeActionKind>>,
) -> CodeActionParams {
    CodeActionParams {
        text_document: TextDocumentIdentifier { uri: uri.clone() },
        range,
        context: CodeActionContext {
            diagnostics,
            only,
            trigger_kind: None,
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    }
}

/// Build the params for `workspace/executeCommand`.
///
/// `command` is the command identifier a `Command` action carries and
/// `arguments` its arguments (empty when the action supplied none).
pub fn execute_command_params(command: String, arguments: Vec<Value>) -> ExecuteCommandParams {
    ExecuteCommandParams {
        command,
        arguments,
        work_done_progress_params: WorkDoneProgressParams::default(),
    }
}

/// Build the params for `textDocument/prepareRename`.
///
/// `lsp-types` 0.97 declares `PrepareRenameRequest::Params =
/// TextDocumentPositionParams`, so there is no distinct `PrepareRenameParams`
/// type; this reuses [`text_document_position_params`].
pub fn prepare_rename_params(uri: &Uri, position: Position) -> TextDocumentPositionParams {
    text_document_position_params(uri, position)
}

/// Build the params for `textDocument/rename`.
///
/// Composes the position with the requested `new_name`; the server answers with
/// the `WorkspaceEdit` that renames the symbol everywhere it is used.
pub fn rename_params(uri: &Uri, position: Position, new_name: String) -> RenameParams {
    RenameParams {
        text_document_position: text_document_position_params(uri, position),
        new_name,
        work_done_progress_params: WorkDoneProgressParams::default(),
    }
}

/// The formatting options the `format` tool sends.
///
/// Fixed in this change request: a tab is four spaces and spaces are preferred
/// over tabs.
pub fn formatting_options() -> FormattingOptions {
    FormattingOptions {
        tab_size: 4,
        insert_spaces: true,
        ..Default::default()
    }
}

/// Build the params for whole-file `textDocument/formatting`.
pub fn document_formatting_params(uri: &Uri) -> DocumentFormattingParams {
    DocumentFormattingParams {
        text_document: TextDocumentIdentifier { uri: uri.clone() },
        options: formatting_options(),
        work_done_progress_params: WorkDoneProgressParams::default(),
    }
}

/// Build the params for `textDocument/rangeFormatting`.
///
/// `range` is the already-converted **0-based** range to format.
pub fn document_range_formatting_params(uri: &Uri, range: Range) -> DocumentRangeFormattingParams {
    DocumentRangeFormattingParams {
        text_document: TextDocumentIdentifier { uri: uri.clone() },
        range,
        options: formatting_options(),
        work_done_progress_params: WorkDoneProgressParams::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_round_trips() {
        let original = request(Id::Number(7), methods::DEFINITION, Some(json!({"a": 1})));
        let bytes = serde_json::to_vec(&original).unwrap();
        let decoded: Request = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(original, decoded);
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["jsonrpc"], "2.0");
        assert_eq!(value["id"], 7);
        assert_eq!(value["method"], "textDocument/definition");
    }

    #[test]
    fn notification_has_no_id_and_round_trips() {
        let original = notification(methods::INITIALIZED, Some(json!({})));
        let value = serde_json::to_value(&original).unwrap();
        assert!(value.get("id").is_none());
        assert_eq!(value["method"], "initialized");
        let decoded: Notification = serde_json::from_value(value).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn success_response_round_trips() {
        let original = Response {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Id::Number(1),
            result: Some(json!({"ok": true})),
            error: None,
        };
        let value = serde_json::to_value(&original).unwrap();
        assert!(value.get("error").is_none());
        let decoded: Response = serde_json::from_value(value).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn error_object_round_trips() {
        let original = Response {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Id::String("abc".to_string()),
            result: None,
            error: Some(ErrorObject {
                code: METHOD_NOT_FOUND,
                message: "method not found".to_string(),
                data: Some(json!({"method": "unknown"})),
            }),
        };
        let value = serde_json::to_value(&original).unwrap();
        assert!(value.get("result").is_none());
        let decoded: Response = serde_json::from_value(value).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn string_ids_deserialize() {
        let value = json!({"jsonrpc": "2.0", "id": "server-1", "result": null});
        let decoded: Response = serde_json::from_value(value).unwrap();
        assert_eq!(decoded.id, Id::String("server-1".to_string()));
    }

    #[test]
    fn did_open_params_serialize_with_the_protocol_field_names() {
        let uri: Uri = "file:///tmp/x.rs".parse().unwrap();
        let params = did_open_params(&uri, "rust".to_string(), 1, "fn main() {}".to_string());
        let value = serde_json::to_value(params).unwrap();
        assert_eq!(value["textDocument"]["languageId"], "rust");
        assert_eq!(value["textDocument"]["version"], 1);
        assert_eq!(value["textDocument"]["uri"], "file:///tmp/x.rs");
    }

    #[test]
    fn text_document_position_params_serialize_with_the_protocol_field_names() {
        let uri: Uri = "file:///tmp/x.rs".parse().unwrap();
        let params = text_document_position_params(&uri, Position::new(4, 7));
        let value = serde_json::to_value(params).unwrap();
        assert_eq!(value["textDocument"]["uri"], "file:///tmp/x.rs");
        assert_eq!(value["position"]["line"], 4);
        assert_eq!(value["position"]["character"], 7);
    }

    #[test]
    fn did_change_params_omit_the_range_for_full_sync() {
        let uri: Uri = "file:///tmp/x.rs".parse().unwrap();
        let params = did_change_params(&uri, 2, "changed".to_string());
        let value = serde_json::to_value(params).unwrap();
        assert_eq!(value["textDocument"]["version"], 2);
        assert_eq!(value["contentChanges"][0]["text"], "changed");
        assert!(value["contentChanges"][0].get("range").is_none());
    }

    #[test]
    fn reference_params_serialize_with_the_protocol_field_names() {
        let uri: Uri = "file:///tmp/x.rs".parse().unwrap();
        let params = reference_params(&uri, Position::new(5, 2), true);
        let value = serde_json::to_value(params).unwrap();
        assert_eq!(value["textDocument"]["uri"], "file:///tmp/x.rs");
        assert_eq!(value["position"]["line"], 5);
        assert_eq!(value["position"]["character"], 2);
        assert_eq!(value["context"]["includeDeclaration"], true);
    }

    #[test]
    fn document_symbol_params_serialize_with_the_protocol_field_names() {
        let uri: Uri = "file:///tmp/x.rs".parse().unwrap();
        let params = document_symbol_params(&uri);
        let value = serde_json::to_value(params).unwrap();
        assert_eq!(value["textDocument"]["uri"], "file:///tmp/x.rs");
    }

    #[test]
    fn workspace_symbol_params_serialize_with_the_protocol_field_names() {
        let params = workspace_symbol_params("main");
        let value = serde_json::to_value(params).unwrap();
        assert_eq!(value["query"], "main");
    }

    #[test]
    fn document_diagnostic_params_serialize_with_the_protocol_field_names() {
        let uri: Uri = "file:///tmp/x.rs".parse().unwrap();
        let params = document_diagnostic_params(&uri);
        let value = serde_json::to_value(params).unwrap();
        assert_eq!(value["textDocument"]["uri"], "file:///tmp/x.rs");
        assert!(value["identifier"].is_null());
        assert!(value["previousResultId"].is_null());
    }

    #[test]
    fn initialize_params_advertise_the_read_tool_client_capabilities() {
        let params = initialize_params(None);
        let value = serde_json::to_value(params).unwrap();
        let text_document = &value["capabilities"]["textDocument"];
        assert!(text_document["hover"].is_object());
        assert!(text_document["references"].is_object());
        assert!(text_document["documentSymbol"].is_object());
        assert!(text_document["publishDiagnostics"].is_object());
        assert!(text_document["diagnostic"].is_object());
        assert!(value["capabilities"]["workspace"]["symbol"].is_object());
    }

    #[test]
    fn code_action_params_serialize_with_the_protocol_field_names() {
        let uri: Uri = "file:///tmp/x.rs".parse().unwrap();
        let range = Range::new(Position::new(1, 2), Position::new(1, 2));
        let diagnostic = Diagnostic {
            range,
            message: "boom".to_string(),
            ..Default::default()
        };
        let params = code_action_params(
            &uri,
            range,
            vec![diagnostic],
            Some(vec![CodeActionKind::QUICKFIX]),
        );
        let value = serde_json::to_value(params).unwrap();
        assert_eq!(value["textDocument"]["uri"], "file:///tmp/x.rs");
        assert_eq!(value["range"]["start"]["line"], 1);
        assert_eq!(value["range"]["start"]["character"], 2);
        assert_eq!(value["context"]["diagnostics"][0]["message"], "boom");
        assert_eq!(value["context"]["only"][0], "quickfix");
    }

    #[test]
    fn code_action_params_omit_only_when_not_requested() {
        let uri: Uri = "file:///tmp/x.rs".parse().unwrap();
        let range = Range::new(Position::new(0, 0), Position::new(0, 0));
        let params = code_action_params(&uri, range, Vec::new(), None);
        let value = serde_json::to_value(params).unwrap();
        assert!(value["context"].get("only").is_none());
        assert_eq!(value["context"]["diagnostics"], json!([]));
    }

    #[test]
    fn execute_command_params_serialize_with_the_protocol_field_names() {
        let params = execute_command_params(
            "fix.thing".to_string(),
            vec![json!({"uri": "file:///tmp/x.rs"})],
        );
        let value = serde_json::to_value(params).unwrap();
        assert_eq!(value["command"], "fix.thing");
        assert_eq!(value["arguments"][0]["uri"], "file:///tmp/x.rs");
    }

    #[test]
    fn initialize_params_advertise_the_code_action_client_capabilities() {
        let params = initialize_params(None);
        let value = serde_json::to_value(params).unwrap();
        let capabilities = &value["capabilities"];
        let code_action = &capabilities["textDocument"]["codeAction"];
        assert!(code_action["codeActionLiteralSupport"].is_object());
        assert_eq!(code_action["dataSupport"], true);
        assert!(code_action["resolveSupport"]["properties"].is_array());

        let workspace_edit = &capabilities["workspace"]["workspaceEdit"];
        assert_eq!(workspace_edit["documentChanges"], true);
        let operations = workspace_edit["resourceOperations"]
            .as_array()
            .expect("resourceOperations is an array");
        for operation in ["create", "rename", "delete"] {
            assert!(
                operations.iter().any(|value| value == operation),
                "resourceOperations must include `{operation}`: {operations:?}"
            );
        }
        assert!(capabilities["workspace"]["executeCommand"].is_object());
    }

    #[test]
    fn rename_params_serialize_with_the_protocol_field_names() {
        let uri: Uri = "file:///tmp/x.rs".parse().unwrap();
        let params = rename_params(&uri, Position::new(1, 8), "renamed".to_string());
        let value = serde_json::to_value(params).unwrap();
        assert_eq!(value["textDocument"]["uri"], "file:///tmp/x.rs");
        assert_eq!(value["position"]["line"], 1);
        assert_eq!(value["position"]["character"], 8);
        assert_eq!(value["newName"], "renamed");
    }

    #[test]
    fn prepare_rename_params_serialize_as_a_position() {
        let uri: Uri = "file:///tmp/x.rs".parse().unwrap();
        let params = prepare_rename_params(&uri, Position::new(0, 3));
        let value = serde_json::to_value(params).unwrap();
        assert_eq!(value["textDocument"]["uri"], "file:///tmp/x.rs");
        assert_eq!(value["position"]["line"], 0);
        assert_eq!(value["position"]["character"], 3);
    }

    #[test]
    fn formatting_params_carry_the_fixed_options() {
        let uri: Uri = "file:///tmp/x.rs".parse().unwrap();
        let value = serde_json::to_value(document_formatting_params(&uri)).unwrap();
        assert_eq!(value["textDocument"]["uri"], "file:///tmp/x.rs");
        assert_eq!(value["options"]["tabSize"], 4);
        assert_eq!(value["options"]["insertSpaces"], true);

        let range = Range::new(Position::new(1, 0), Position::new(1, 8));
        let value = serde_json::to_value(document_range_formatting_params(&uri, range)).unwrap();
        assert_eq!(value["range"]["start"]["line"], 1);
        assert_eq!(value["range"]["end"]["character"], 8);
        assert_eq!(value["options"]["tabSize"], 4);
    }

    #[test]
    fn initialize_params_advertise_the_write_tool_client_capabilities() {
        let params = initialize_params(None);
        let value = serde_json::to_value(params).unwrap();
        let text_document = &value["capabilities"]["textDocument"];
        assert_eq!(text_document["rename"]["prepareSupport"], true);
        assert!(text_document["formatting"].is_object());
        assert!(text_document["rangeFormatting"].is_object());
    }
}
