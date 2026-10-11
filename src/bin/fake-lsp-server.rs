//! A tiny, dependency-light fake LSP server used by the process-level tests.
//!
//! It speaks `Content-Length` framing on stdio and answers the read/explore
//! requests the adapter issues — `initialize` (with a capabilities payload that
//! depends on the mode), `textDocument/definition`, `textDocument/references`,
//! `textDocument/hover`, `textDocument/documentSymbol`, `workspace/symbol`, and
//! `textDocument/diagnostic` — plus `shutdown`. It also serves the code-action
//! requests: `textDocument/codeAction` (a deterministic mixed list),
//! `codeAction/resolve`, and `workspace/executeCommand` (which mutates the target
//! file so execution is observable), and the write-tool requests:
//! `textDocument/prepareRename`, `textDocument/rename` (a two-file
//! `WorkspaceEdit`), `textDocument/formatting`, and
//! `textDocument/rangeFormatting`. Any other request is answered with a JSON-RPC
//! `MethodNotFound` error.
//!
//! The first CLI argument selects a mode:
//!
//! - absent or `default` — advertise every read capability, the code-action
//!   capabilities, and the rename/formatting capabilities, and answer pull
//!   diagnostics, code actions, and the write-tool requests;
//! - `push` — advertise no `diagnosticProvider`, and push
//!   `textDocument/publishDiagnostics` in response to `textDocument/didOpen`;
//! - `none` — advertise none of the read, code-action, or write capabilities, so
//!   the capability gates trip.
//!
//! It exists only so `cargo test` can exercise the compiled `lsp-mcp` binary end
//! to end without a real language server or network access.

use std::io::{self, BufRead, Write};

use serde_json::{json, Value};

/// How the fake server advertises and answers diagnostics, and which read
/// capabilities it advertises at all.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Advertise everything; answer pull diagnostics.
    Default,
    /// Advertise everything except pull diagnostics; push on `didOpen`.
    Push,
    /// Advertise everything including pull diagnostics, but answer pull with an
    /// empty report and push a report on `didOpen`. This mirrors the split a
    /// real server (e.g. rust-analyzer) shows between the native diagnostics it
    /// answers pull with and the `cargo check`/clippy diagnostics it only pushes.
    PullAndPush,
    /// Advertise none of the read capabilities.
    None,
}

impl Mode {
    fn from_args() -> Self {
        match std::env::args().nth(1).as_deref() {
            Some("push") => Mode::Push,
            Some("pull-and-push") => Mode::PullAndPush,
            Some("none") => Mode::None,
            _ => Mode::Default,
        }
    }
}

struct Server {
    mode: Mode,
    /// The URI seen in the most recent document request, used to answer
    /// `workspace/symbol`, which carries no document.
    last_uri: Option<String>,
}

fn main() {
    let stdin = io::stdin();
    let mut reader = io::BufReader::new(stdin.lock());
    let stdout = io::stdout();
    let mut writer = io::BufWriter::new(stdout.lock());

    let mut server = Server {
        mode: Mode::from_args(),
        last_uri: None,
    };

    while let Some(body) = read_frame(&mut reader) {
        let message: Value = match serde_json::from_slice(&body) {
            Ok(message) => message,
            Err(_) => continue,
        };
        server.handle(&message, &mut writer);
    }
    let _ = writer.flush();
}

fn read_frame<R: BufRead>(reader: &mut R) -> Option<Vec<u8>> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some(value) = trimmed.strip_prefix("Content-Length:") {
            content_length = value.trim().parse().ok();
        }
    }
    let length = content_length?;
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).ok()?;
    Some(body)
}

impl Server {
    fn handle(&mut self, message: &Value, writer: &mut impl Write) {
        let method = message.get("method").and_then(Value::as_str);
        let id = message.get("id").cloned();

        // Remember the document URI from any request or notification that
        // carries one, so `workspace/symbol` can point at a real file.
        if let Some(uri) = message
            .get("params")
            .and_then(|params| params.get("textDocument"))
            .and_then(|document| document.get("uri"))
            .and_then(Value::as_str)
        {
            self.last_uri = Some(uri.to_string());
        }

        match (method, id) {
            (Some("initialize"), Some(id)) => {
                write_value(
                    writer,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {"capabilities": self.capabilities()}
                    }),
                );
            }
            (Some("textDocument/definition"), Some(id)) => {
                write_value(
                    writer,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": self.location(0, 0, 3)
                    }),
                );
            }
            (Some("textDocument/references"), Some(id)) => {
                write_value(
                    writer,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": [
                            self.location(0, 0, 3),
                            self.location(1, 8, 14)
                        ]
                    }),
                );
            }
            (Some("textDocument/hover"), Some(id)) => {
                write_value(
                    writer,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "contents": {"kind": "markdown", "value": "fn main() -> ()"},
                            "range": {
                                "start": {"line": 0, "character": 3},
                                "end": {"line": 0, "character": 7}
                            }
                        }
                    }),
                );
            }
            (Some("textDocument/documentSymbol"), Some(id)) => {
                write_value(
                    writer,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": [{
                            "name": "main",
                            "kind": 12,
                            "range": {"start": {"line": 0, "character": 0},
                                      "end": {"line": 2, "character": 1}},
                            "selectionRange": {"start": {"line": 0, "character": 3},
                                               "end": {"line": 0, "character": 7}},
                            "children": [{
                                "name": "answer",
                                "kind": 13,
                                "detail": "let answer = 42",
                                "range": {"start": {"line": 1, "character": 4},
                                          "end": {"line": 1, "character": 21}},
                                "selectionRange": {"start": {"line": 1, "character": 8},
                                                   "end": {"line": 1, "character": 14}}
                            }]
                        }]
                    }),
                );
            }
            (Some("workspace/symbol"), Some(id)) => {
                let uri = self
                    .last_uri
                    .clone()
                    .unwrap_or_else(|| "file:///workspace/main.rs".to_string());
                write_value(
                    writer,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": [{
                            "name": "main",
                            "kind": 12,
                            "location": {
                                "uri": uri,
                                "range": {"start": {"line": 0, "character": 3},
                                          "end": {"line": 0, "character": 7}}
                            }
                        }]
                    }),
                );
            }
            (Some("textDocument/diagnostic"), Some(id)) => {
                write_value(
                    writer,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "kind": "full",
                            "items": [self.diagnostic()]
                        }
                    }),
                );
            }
            (Some("textDocument/codeAction"), Some(id)) => {
                write_value(
                    writer,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": self.code_actions()
                    }),
                );
            }
            (Some("codeAction/resolve"), Some(id)) => {
                write_value(
                    writer,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": self.resolved_action()
                    }),
                );
            }
            (Some("workspace/executeCommand"), Some(id)) => {
                let command = message
                    .get("params")
                    .and_then(|params| params.get("command"))
                    .and_then(Value::as_str);
                if command == Some("fake.apply") {
                    if let Some(path) = self.path() {
                        let mut text = std::fs::read_to_string(&path).unwrap_or_default();
                        text.push_str("// executed\n");
                        let _ = std::fs::write(&path, text);
                    }
                }
                write_value(writer, &json!({"jsonrpc": "2.0", "id": id, "result": null}));
            }
            (Some("textDocument/prepareRename"), Some(id)) => {
                let position = message
                    .get("params")
                    .and_then(|params| params.get("position"));
                write_value(
                    writer,
                    &json!({"jsonrpc": "2.0", "id": id, "result": prepared_rename(position)}),
                );
            }
            (Some("textDocument/rename"), Some(id)) => {
                let new_name = message
                    .get("params")
                    .and_then(|params| params.get("newName"))
                    .and_then(Value::as_str)
                    .unwrap_or("renamed");
                let mut changes = serde_json::Map::new();
                changes.insert(self.uri(), rename_edit(new_name));
                changes.insert(self.sibling_uri(), rename_edit(new_name));
                write_value(
                    writer,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {"changes": Value::Object(changes)}
                    }),
                );
            }
            (Some("textDocument/formatting"), Some(id)) => {
                let result = self
                    .path()
                    .and_then(|path| std::fs::read_to_string(path).ok())
                    .map(|text| whole_document_edit(&text))
                    .unwrap_or(Value::Null);
                write_value(
                    writer,
                    &json!({"jsonrpc": "2.0", "id": id, "result": result}),
                );
            }
            (Some("textDocument/rangeFormatting"), Some(id)) => {
                let range = message
                    .get("params")
                    .and_then(|params| params.get("range"))
                    .cloned()
                    .unwrap_or(Value::Null);
                write_value(
                    writer,
                    &json!({"jsonrpc": "2.0", "id": id, "result": range_edit(&range)}),
                );
            }
            (Some("shutdown"), Some(id)) => {
                write_value(writer, &json!({"jsonrpc": "2.0", "id": id, "result": null}))
            }
            (Some("textDocument/didOpen"), None)
                if matches!(self.mode, Mode::Push | Mode::PullAndPush) =>
            {
                let uri = self
                    .last_uri
                    .clone()
                    .unwrap_or_else(|| "file:///unknown".to_string());
                // `PullAndPush` also pushes a check-only diagnostic, so a merge
                // can be told apart from a plain pull.
                let diagnostics = if self.mode == Mode::PullAndPush {
                    json!([self.diagnostic(), self.check_diagnostic()])
                } else {
                    json!([self.diagnostic()])
                };
                write_value(
                    writer,
                    &json!({
                        "jsonrpc": "2.0",
                        "method": "textDocument/publishDiagnostics",
                        "params": {"uri": uri, "diagnostics": diagnostics}
                    }),
                );
            }
            (Some(_), Some(id)) => write_value(
                writer,
                &json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {"code": -32601, "message": "method not found"}
                }),
            ),
            // Other notifications (no id) need no response.
            _ => {}
        }
    }

    /// The capabilities to advertise for this mode.
    fn capabilities(&self) -> Value {
        if self.mode == Mode::None {
            return json!({"textDocumentSync": 1});
        }
        let mut capabilities = json!({
            "definitionProvider": true,
            "hoverProvider": true,
            "referencesProvider": true,
            "documentSymbolProvider": true,
            "workspaceSymbolProvider": true,
            "codeActionProvider": {
                "codeActionKinds": ["quickfix", "refactor"],
                "resolveProvider": true
            },
            "executeCommandProvider": {"commands": ["fake.apply"]},
            "renameProvider": {"prepareProvider": true},
            "documentFormattingProvider": true,
            "documentRangeFormattingProvider": true,
            "textDocumentSync": 1
        });
        if matches!(self.mode, Mode::Default | Mode::PullAndPush) {
            capabilities["diagnosticProvider"] = json!({
                "interFileDependencies": false,
                "workspaceDiagnostics": false
            });
        }
        capabilities
    }

    /// The document URI seen so far, defaulting to a placeholder.
    fn uri(&self) -> String {
        self.last_uri
            .clone()
            .unwrap_or_else(|| "file:///unknown".to_string())
    }

    /// The filesystem path behind the last-seen `file://` URI, if any.
    fn path(&self) -> Option<String> {
        self.last_uri
            .as_deref()
            .and_then(|uri| uri.strip_prefix("file://"))
            .map(str::to_string)
    }

    /// The deterministic sibling URI for the last-seen document: its `file://`
    /// stem gets a `-sibling` suffix, so a rename's second file is addressable.
    fn sibling_uri(&self) -> String {
        let uri = self.uri();
        match uri.strip_suffix(".rs") {
            Some(stem) => format!("{stem}-sibling.rs"),
            None => format!("{uri}-sibling"),
        }
    }

    /// A deterministic mixed code-action list: a direct edit, a resolve-only
    /// action, and a bare command.
    fn code_actions(&self) -> Value {
        let mut changes = serde_json::Map::new();
        changes.insert(
            self.uri(),
            json!([{
                "range": {"start": {"line": 1, "character": 17},
                          "end": {"line": 1, "character": 19}},
                "newText": "43"
            }]),
        );
        json!([
            {
                "title": "Fix: replace 42",
                "kind": "quickfix",
                "isPreferred": true,
                "edit": {"changes": Value::Object(changes)}
            },
            {
                "title": "Deferred fix",
                "kind": "quickfix",
                "data": {"fake": "deferred"}
            },
            {
                "title": "Run the fix",
                "command": "fake.apply",
                "arguments": []
            }
        ])
    }

    /// The resolved form of the resolve-only action: a `documentChanges` edit.
    fn resolved_action(&self) -> Value {
        json!({
            "title": "Deferred fix",
            "kind": "quickfix",
            "edit": {
                "documentChanges": [{
                    "textDocument": {"uri": self.uri()},
                    "edits": [{
                        "range": {"start": {"line": 1, "character": 8},
                                  "end": {"line": 1, "character": 14}},
                        "newText": "result"
                    }]
                }]
            }
        })
    }

    /// A location in the last-seen document at a fixed range.
    fn location(&self, line: u32, start: u32, end: u32) -> Value {
        json!({
            "uri": self.last_uri.clone().unwrap_or_else(|| "file:///unknown".to_string()),
            "range": {
                "start": {"line": line, "character": start},
                "end": {"line": line, "character": end}
            }
        })
    }

    /// A single canned diagnostic, the native one; a server that both answers
    /// pull and pushes returns it on both channels.
    fn diagnostic(&self) -> Value {
        json!({
            "range": {
                "start": {"line": 1, "character": 8},
                "end": {"line": 1, "character": 14}
            },
            "severity": 1,
            "code": "E0001",
            "source": "fake",
            "message": "unused variable"
        })
    }

    /// A single canned check (clippy-style) diagnostic, distinct from the native
    /// one and delivered only by push.
    fn check_diagnostic(&self) -> Value {
        json!({
            "range": {
                "start": {"line": 0, "character": 4},
                "end": {"line": 0, "character": 10}
            },
            "severity": 2,
            "code": "clippy::mutable_key_type",
            "source": "clippy",
            "message": "mutable key type"
        })
    }
}

fn write_value(writer: &mut impl Write, value: &Value) {
    let body = serde_json::to_vec(value).expect("serialize a JSON-RPC message");
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    writer.write_all(header.as_bytes()).expect("write header");
    writer.write_all(&body).expect("write body");
    writer.flush().expect("flush");
}

/// The prepareRename answer for a position: a placeholder range when the position
/// names the `answer` symbol (0-based line 1, characters 8..=14), `null`
/// otherwise.
fn prepared_rename(position: Option<&Value>) -> Value {
    let line = position.and_then(|p| p.get("line")).and_then(Value::as_u64);
    let character = position
        .and_then(|p| p.get("character"))
        .and_then(Value::as_u64);
    if line == Some(1) && matches!(character, Some(8..=14)) {
        json!({
            "range": {"start": {"line": 1, "character": 8},
                      "end": {"line": 1, "character": 14}},
            "placeholder": "answer"
        })
    } else {
        Value::Null
    }
}

/// A rename edit for one file: replace `answer` (0-based line 1, characters
/// 8..14) with `new_name`.
fn rename_edit(new_name: &str) -> Value {
    json!([{
        "range": {"start": {"line": 1, "character": 8},
                  "end": {"line": 1, "character": 14}},
        "newText": new_name
    }])
}

/// A whole-document edit: replace the entire file with its content plus a
/// deterministic `// formatted` marker, computed from the file on disk.
fn whole_document_edit(text: &str) -> Value {
    let end_line = text.matches('\n').count() as u64;
    let formatted = format!("{text}// formatted\n");
    json!([{
        "range": {"start": {"line": 0, "character": 0},
                  "end": {"line": end_line, "character": 0}},
        "newText": formatted
    }])
}

/// A single edit limited to `range`, replacing it with a marker.
fn range_edit(range: &Value) -> Value {
    json!([{ "range": range, "newText": "// range formatted" }])
}
