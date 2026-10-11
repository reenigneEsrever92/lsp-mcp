//! Process-level, end-to-end MCP tests for the compiled `lsp-mcp` binary.
//!
//! Each test spawns the real `lsp-mcp` with the `fake-lsp-server` helper named as
//! the server (and an optional mode argument forwarded to it after `--`) and acts
//! as an MCP client over the child's stdio, speaking **newline-delimited**
//! JSON-RPC. They exercise the acceptance
//! criteria directly: listing all ten tools (name, description, input schema),
//! structured results carrying URI/line/column/context and the raw LSP payload
//! for each tool, diagnostics in both pull and push modes, code-action listing
//! and applying (a `WorkspaceEdit` writes a temp file, a `Command` runs), rename
//! and format writing their edits to disk, an unsupported capability yielding an
//! `isError` result, and stdout carrying nothing but MCP messages while logs go
//! to stderr.

mod common;

use common::{expected_uri, sibling_path, temp_source_path, McpClient};
use serde_json::{json, Value};

/// Assert a structured result carries a URI, 1-based line/column, context, and
/// a raw payload.
#[test]
fn lists_tools_and_returns_structured_results() {
    let path = temp_source_path("read-tools");
    let uri = expected_uri(&path);

    let mut client = McpClient::spawn(&[]);
    client.initialize();

    // --- tools/list: all ten tools with descriptions and schemas ----------
    let list = client.request("tools/list", json!({}));
    let tools = list["result"]["tools"]
        .as_array()
        .expect("tools/list returns an array");
    for name in [
        "definition",
        "references",
        "hover",
        "documentSymbol",
        "workspaceSymbol",
        "diagnostics",
        "codeAction",
        "applyCodeAction",
        "rename",
        "format",
    ] {
        let tool = tools
            .iter()
            .find(|tool| tool["name"] == name)
            .unwrap_or_else(|| panic!("the `{name}` tool is listed: {list}"));
        assert!(
            !tool["description"].as_str().unwrap_or("").is_empty(),
            "`{name}` must carry a description: {tool}"
        );
        assert!(
            tool["inputSchema"]["properties"].is_object(),
            "`{name}` must carry an input schema: {tool}"
        );
    }
    let properties = |tools: &Value, name: &str| -> Value {
        tools
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == name)
            .unwrap()["inputSchema"]["properties"]
            .clone()
    };
    for field in ["file", "line", "column"] {
        assert!(
            properties(&list["result"]["tools"], "definition")[field].is_object(),
            "definition is missing `{field}`"
        );
        assert!(
            properties(&list["result"]["tools"], "references")[field].is_object(),
            "references is missing `{field}`"
        );
        assert!(
            properties(&list["result"]["tools"], "hover")[field].is_object(),
            "hover is missing `{field}`"
        );
    }
    assert!(
        properties(&list["result"]["tools"], "references")["include_declaration"].is_object(),
        "references is missing `include_declaration`"
    );
    for name in ["documentSymbol", "diagnostics"] {
        assert!(
            properties(&list["result"]["tools"], name)["file"].is_object(),
            "{name} is missing `file`"
        );
    }
    assert!(
        properties(&list["result"]["tools"], "workspaceSymbol")["query"].is_object(),
        "workspaceSymbol is missing `query`"
    );
    for field in ["file", "line", "column", "include_diagnostics", "only"] {
        assert!(
            properties(&list["result"]["tools"], "codeAction")[field].is_object(),
            "codeAction is missing `{field}`"
        );
    }
    for field in ["file", "line", "column", "index", "title"] {
        assert!(
            properties(&list["result"]["tools"], "applyCodeAction")[field].is_object(),
            "applyCodeAction is missing `{field}`"
        );
    }
    for field in ["file", "line", "column", "new_name"] {
        assert!(
            properties(&list["result"]["tools"], "rename")[field].is_object(),
            "rename is missing `{field}`"
        );
    }
    for field in ["file", "range"] {
        assert!(
            properties(&list["result"]["tools"], "format")[field].is_object(),
            "format is missing `{field}`"
        );
    }

    // --- definition ------------------------------------------------------
    let result = client.call(
        "definition",
        json!({"file": path.to_string_lossy(), "line": 1, "column": 1}),
    );
    assert_eq!(result["isError"], json!(false), "definition: {result}");
    let locations = result["structuredContent"]["locations"]
        .as_array()
        .expect("definition carries locations");
    assert_eq!(locations.len(), 1);
    assert_eq!(locations[0]["uri"], json!(uri));
    assert_eq!(locations[0]["line"], json!(1));
    assert!(locations[0]["context"]
        .as_str()
        .unwrap_or("")
        .contains("fn main()"));

    // --- references ------------------------------------------------------
    let result = client.call(
        "references",
        json!({"file": path.to_string_lossy(), "line": 1, "column": 1}),
    );
    assert_eq!(result["isError"], json!(false), "references: {result}");
    let locations = result["structuredContent"]["locations"]
        .as_array()
        .expect("references carries locations");
    assert_eq!(locations.len(), 2, "references: {result}");
    assert_eq!(locations[0]["uri"], json!(uri));
    assert!(locations[0]["line"].is_number());
    assert!(locations[0]["column"].is_number());
    assert!(
        !locations[1]["context"].as_str().unwrap_or("").is_empty(),
        "references must include code context: {result}"
    );
    assert!(
        result["structuredContent"].get("raw").is_some(),
        "references must include the raw payload: {result}"
    );

    // --- hover -----------------------------------------------------------
    let result = client.call(
        "hover",
        json!({"file": path.to_string_lossy(), "line": 1, "column": 1}),
    );
    assert_eq!(result["isError"], json!(false), "hover: {result}");
    let hover = &result["structuredContent"];
    assert_eq!(hover["uri"], json!(uri));
    assert!(
        hover["hover"]["contents"]
            .as_str()
            .unwrap_or("")
            .contains("fn main()"),
        "hover contents must be rendered: {result}"
    );
    assert_eq!(hover["hover"]["range"]["line"], json!(1));
    assert!(hover["location"]["line"].is_number());
    assert!(hover.get("raw").is_some());

    // --- documentSymbol --------------------------------------------------
    let result = client.call("documentSymbol", json!({"file": path.to_string_lossy()}));
    assert_eq!(result["isError"], json!(false), "documentSymbol: {result}");
    let symbols = result["structuredContent"]["symbols"]
        .as_array()
        .expect("documentSymbol carries symbols");
    assert_eq!(symbols.len(), 2, "documentSymbol: {result}");
    assert_eq!(symbols[0]["name"], json!("main"));
    assert_eq!(symbols[0]["kind"], json!("function"));
    assert_eq!(symbols[1]["name"], json!("answer"));
    assert_eq!(symbols[0]["location"]["uri"], json!(uri));
    assert!(symbols[0]["location"]["line"].is_number());
    assert!(
        !symbols[0]["location"]["context"]
            .as_str()
            .unwrap_or("")
            .is_empty(),
        "documentSymbol must include context: {result}"
    );

    // --- workspaceSymbol -------------------------------------------------
    let result = client.call("workspaceSymbol", json!({"query": "main"}));
    assert_eq!(result["isError"], json!(false), "workspaceSymbol: {result}");
    let symbols = result["structuredContent"]["symbols"]
        .as_array()
        .expect("workspaceSymbol carries symbols");
    assert_eq!(symbols.len(), 1, "workspaceSymbol: {result}");
    assert_eq!(symbols[0]["name"], json!("main"));
    assert_eq!(symbols[0]["uri"], json!(uri));
    assert!(symbols[0]["location"]["line"].is_number());
    assert!(
        !symbols[0]["location"]["context"]
            .as_str()
            .unwrap_or("")
            .is_empty(),
        "workspaceSymbol must include context for its file: {result}"
    );

    // --- diagnostics (pull) ---------------------------------------------
    let result = client.call("diagnostics", json!({"file": path.to_string_lossy()}));
    assert_eq!(result["isError"], json!(false), "diagnostics: {result}");
    let structured = &result["structuredContent"];
    assert_eq!(structured["mechanism"], json!("pull"));
    let diagnostics = structured["diagnostics"]
        .as_array()
        .expect("diagnostics carries diagnostics");
    assert_eq!(diagnostics.len(), 1, "diagnostics: {result}");
    assert_eq!(diagnostics[0]["severity"], json!("error"));
    assert_eq!(diagnostics[0]["message"], json!("unused variable"));
    assert_eq!(diagnostics[0]["location"]["uri"], json!(uri));
    assert!(diagnostics[0]["location"]["line"].is_number());

    // --- invalid arguments are tool errors, not crashes ------------------
    let zero_line = client.call(
        "references",
        json!({"file": path.to_string_lossy(), "line": 0, "column": 1}),
    );
    assert_eq!(zero_line["isError"], json!(true));

    let missing_file = client.call(
        "hover",
        json!({"file": "/definitely/not/real-lsp-mcp-file.rs", "line": 1, "column": 1}),
    );
    assert_eq!(missing_file["isError"], json!(true));

    // --- shutdown and stdout/stderr discipline ---------------------------
    let (code, stderr) = client.finish();
    assert_eq!(code, 0, "expected a clean exit; stderr:\n{stderr}");
    assert!(
        stderr.contains("capabilities captured"),
        "the handshake summary belongs on stderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("panicked"),
        "the server must never panic:\n{stderr}"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn returns_pushed_diagnostics() {
    let path = temp_source_path("push-diagnostics");
    let uri = expected_uri(&path);

    let mut client = McpClient::spawn(&["push"]);
    client.initialize();

    let result = client.call("diagnostics", json!({"file": path.to_string_lossy()}));
    assert_eq!(result["isError"], json!(false), "diagnostics: {result}");
    let structured = &result["structuredContent"];
    assert_eq!(
        structured["mechanism"],
        json!("publish"),
        "push mode must use the publish mechanism: {result}"
    );
    let diagnostics = structured["diagnostics"]
        .as_array()
        .expect("diagnostics carries diagnostics");
    assert_eq!(diagnostics.len(), 1, "diagnostics: {result}");
    assert_eq!(diagnostics[0]["message"], json!("unused variable"));
    assert_eq!(diagnostics[0]["location"]["uri"], json!(uri));
    assert!(
        !diagnostics[0]["location"]["context"]
            .as_str()
            .unwrap_or("")
            .is_empty(),
        "pushed diagnostics must include context: {result}"
    );
    assert!(structured.get("raw").is_some());

    let (code, stderr) = client.finish();
    assert_eq!(code, 0, "expected a clean exit; stderr:\n{stderr}");
    assert!(
        !stderr.contains("panicked"),
        "the server must never panic:\n{stderr}"
    );

    let _ = std::fs::remove_file(&path);
}

/// A server may answer pull diagnostics with the *native* report while
/// delivering the diagnostics that matter — `cargo check`/clippy output, say —
/// only through `publishDiagnostics`, and it may include the native ones on both
/// channels. The tool must merge the two without dropping or duplicating.
#[test]
fn merges_diagnostics_pushed_alongside_pull() {
    let path = temp_source_path("pull-and-push-diagnostics");
    let uri = expected_uri(&path);

    let mut client = McpClient::spawn(&["pull-and-push"]);
    client.initialize();

    let result = client.call("diagnostics", json!({"file": path.to_string_lossy()}));
    assert_eq!(result["isError"], json!(false), "diagnostics: {result}");
    let diagnostics = result["structuredContent"]["diagnostics"]
        .as_array()
        .expect("diagnostics carries diagnostics");
    assert!(
        diagnostics
            .iter()
            .any(|d| d["message"] == json!("mutable key type")),
        "a diagnostic the server only pushes must not be dropped when it also \
         advertises pull: {result}"
    );
    assert!(
        diagnostics
            .iter()
            .any(|d| d["message"] == json!("unused variable")),
        "the pull diagnostics must be kept: {result}"
    );
    assert_eq!(
        diagnostics
            .iter()
            .filter(|d| d["message"] == json!("unused variable"))
            .count(),
        1,
        "a diagnostic present on both channels must not be duplicated: {result}"
    );
    assert_eq!(diagnostics.len(), 2, "diagnostics: {result}");
    assert_eq!(diagnostics[0]["location"]["uri"], json!(uri));

    let (code, stderr) = client.finish();
    assert_eq!(code, 0, "expected a clean exit; stderr:\n{stderr}");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn lists_and_applies_code_actions() {
    let edit_path = temp_source_path("code-action-edit");
    let resolve_path = temp_source_path("code-action-resolve");
    let command_path = temp_source_path("code-action-command");

    let mut client = McpClient::spawn(&[]);
    client.initialize();

    // --- codeAction lists the three actions with identities --------------
    let result = client.call(
        "codeAction",
        json!({"file": edit_path.to_string_lossy(), "line": 1, "column": 1}),
    );
    assert_eq!(result["isError"], json!(false), "codeAction: {result}");
    let actions = result["structuredContent"]["actions"]
        .as_array()
        .expect("codeAction carries actions");
    assert_eq!(actions.len(), 3, "codeAction: {result}");
    assert_eq!(actions[0]["index"], json!(0));
    assert_eq!(actions[0]["title"], json!("Fix: replace 42"));
    assert!(actions[0]["hasEdit"].as_bool().unwrap_or(false));
    assert_eq!(actions[1]["index"], json!(1));
    assert_eq!(actions[1]["title"], json!("Deferred fix"));
    assert!(!actions[1]["hasEdit"].as_bool().unwrap_or(true));
    assert!(!actions[1]["hasCommand"].as_bool().unwrap_or(true));
    assert_eq!(actions[2]["title"], json!("Run the fix"));
    assert_eq!(actions[2]["command"], json!("fake.apply"));
    assert!(result["structuredContent"].get("raw").is_some());

    // --- apply the direct-edit action (index 0) --------------------------
    let result = client.call(
        "applyCodeAction",
        json!({
            "file": edit_path.to_string_lossy(),
            "line": 1,
            "column": 1,
            "index": 0,
            "title": "Fix: replace 42"
        }),
    );
    assert_eq!(result["isError"], json!(false), "apply direct: {result}");
    let changed = result["structuredContent"]["filesChanged"]
        .as_array()
        .expect("applyCodeAction carries filesChanged");
    assert!(
        changed
            .iter()
            .any(|path| path == &json!(edit_path.to_string_lossy())),
        "filesChanged must include the temp path: {result}"
    );
    let text = std::fs::read_to_string(&edit_path).expect("read the edited file");
    assert!(
        text.contains("= 43;"),
        "the direct edit must reach disk: {text}"
    );

    // --- apply the resolve-only action (index 1) -------------------------
    let result = client.call(
        "applyCodeAction",
        json!({
            "file": resolve_path.to_string_lossy(),
            "line": 1,
            "column": 1,
            "index": 1
        }),
    );
    assert_eq!(result["isError"], json!(false), "apply resolve: {result}");
    let text = std::fs::read_to_string(&resolve_path).expect("read the resolved file");
    assert!(
        text.contains("let result"),
        "the resolved edit must reach disk: {text}"
    );

    // --- apply the command action (index 2) ------------------------------
    let result = client.call(
        "applyCodeAction",
        json!({
            "file": command_path.to_string_lossy(),
            "line": 1,
            "column": 1,
            "index": 2
        }),
    );
    assert_eq!(result["isError"], json!(false), "apply command: {result}");
    assert_eq!(
        result["structuredContent"]["executedCommand"],
        json!("fake.apply")
    );
    let text = std::fs::read_to_string(&command_path).expect("read the command file");
    assert!(
        text.contains("// executed"),
        "the command must mutate the file: {text}"
    );

    // --- an out-of-range index is a clear error --------------------------
    let result = client.call(
        "applyCodeAction",
        json!({
            "file": command_path.to_string_lossy(),
            "line": 1,
            "column": 1,
            "index": 9
        }),
    );
    assert_eq!(result["isError"], json!(true), "out-of-range: {result}");
    let message = result["content"][0]["text"].as_str().unwrap_or("");
    assert!(
        message.contains("index"),
        "the error must name the index: {message:?}"
    );

    // --- a title that does not match is refused --------------------------
    let result = client.call(
        "applyCodeAction",
        json!({
            "file": command_path.to_string_lossy(),
            "line": 1,
            "column": 1,
            "index": 0,
            "title": "Not the title"
        }),
    );
    assert_eq!(result["isError"], json!(true), "title guard: {result}");

    let (code, stderr) = client.finish();
    assert_eq!(code, 0, "expected a clean exit; stderr:\n{stderr}");
    assert!(
        !stderr.contains("panicked"),
        "the server must never panic:\n{stderr}"
    );

    let _ = std::fs::remove_file(&edit_path);
    let _ = std::fs::remove_file(&resolve_path);
    let _ = std::fs::remove_file(&command_path);
}

#[test]
fn renames_a_symbol_across_files() {
    let path = temp_source_path("rename");
    let sibling = sibling_path(&path);
    std::fs::write(&sibling, "fn main() {\n    let answer = 42;\n}\n")
        .expect("write the sibling file");

    let mut client = McpClient::spawn(&[]);
    client.initialize();

    // --- rename updates both files and reports them ----------------------
    let result = client.call(
        "rename",
        json!({
            "file": path.to_string_lossy(),
            "line": 2,
            "column": 9,
            "new_name": "total"
        }),
    );
    assert_eq!(result["isError"], json!(false), "rename: {result}");
    let structured = &result["structuredContent"];
    assert_eq!(structured["oldName"], json!("answer"));
    assert_eq!(structured["newName"], json!("total"));
    let changed = structured["filesChanged"]
        .as_array()
        .expect("rename carries filesChanged");
    assert_eq!(changed.len(), 2, "both files must be reported: {result}");

    let main_text = std::fs::read_to_string(&path).expect("read the main file");
    assert!(
        main_text.contains("let total = 42;"),
        "the main file must be renamed on disk: {main_text}"
    );
    let sibling_text = std::fs::read_to_string(&sibling).expect("read the sibling file");
    assert!(
        sibling_text.contains("let total = 42;"),
        "the sibling file must be renamed on disk: {sibling_text}"
    );

    // --- a non-renameable position errors and writes nothing -------------
    let before = std::fs::read_to_string(&path).expect("read the main file again");
    let result = client.call(
        "rename",
        json!({
            "file": path.to_string_lossy(),
            "line": 1,
            "column": 1,
            "new_name": "nope"
        }),
    );
    assert_eq!(result["isError"], json!(true), "non-renameable: {result}");
    let message = result["content"][0]["text"].as_str().unwrap_or("");
    assert!(
        message.contains("renameable"),
        "the error must name the position: {message:?}"
    );
    let after = std::fs::read_to_string(&path).expect("read the main file after");
    assert_eq!(
        before, after,
        "a non-renameable position must write nothing"
    );

    let (code, stderr) = client.finish();
    assert_eq!(code, 0, "expected a clean exit; stderr:\n{stderr}");
    assert!(
        !stderr.contains("panicked"),
        "the server must never panic:\n{stderr}"
    );

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&sibling);
}

#[test]
fn formats_a_whole_file_and_a_range() {
    let whole = temp_source_path("format-whole");
    let ranged = temp_source_path("format-range");

    let mut client = McpClient::spawn(&[]);
    client.initialize();

    // --- whole-file formatting writes the formatted content --------------
    let result = client.call("format", json!({"file": whole.to_string_lossy()}));
    assert_eq!(result["isError"], json!(false), "format whole: {result}");
    let structured = &result["structuredContent"];
    assert_eq!(
        structured["file"],
        json!(whole.to_string_lossy().to_string())
    );
    assert!(
        structured["appliedEdits"].as_u64().unwrap_or(0) >= 1,
        "whole-file formatting applies an edit: {result}"
    );
    let text = std::fs::read_to_string(&whole).expect("read the formatted file");
    assert!(
        text.contains("// formatted"),
        "the whole-file edit must reach disk: {text}"
    );
    assert!(
        text.contains("fn main() {"),
        "the original content must survive: {text}"
    );

    // --- range formatting changes only the range -------------------------
    let original = std::fs::read_to_string(&ranged).expect("read the range file");
    let result = client.call(
        "format",
        json!({
            "file": ranged.to_string_lossy(),
            "range": {"line": 2, "column": 1, "end_line": 2, "end_column": 20}
        }),
    );
    assert_eq!(result["isError"], json!(false), "format range: {result}");
    let structured = &result["structuredContent"];
    assert_eq!(structured["range"]["line"], json!(2));
    let text = std::fs::read_to_string(&ranged).expect("read the range-formatted file");
    assert!(
        text.contains("// range formatted"),
        "the range edit must reach disk: {text}"
    );
    assert!(
        text.contains("fn main() {"),
        "content outside the range is unchanged: {text}"
    );
    assert!(
        !text.contains("answer"),
        "only the range changed, removing the original line: {text}"
    );
    assert_ne!(text, original, "the range formatting must change the file");

    let (code, stderr) = client.finish();
    assert_eq!(code, 0, "expected a clean exit; stderr:\n{stderr}");
    assert!(
        !stderr.contains("panicked"),
        "the server must never panic:\n{stderr}"
    );

    let _ = std::fs::remove_file(&whole);
    let _ = std::fs::remove_file(&ranged);
}

#[test]
fn reports_an_unsupported_capability_clearly() {
    let path = temp_source_path("no-capabilities");

    let mut client = McpClient::spawn(&["none"]);
    client.initialize();

    let file = path.to_string_lossy().to_string();
    let cases = [
        (
            "references",
            json!({"file": file, "line": 1, "column": 1}),
            "referencesProvider",
        ),
        (
            "hover",
            json!({"file": file, "line": 1, "column": 1}),
            "hoverProvider",
        ),
        (
            "documentSymbol",
            json!({"file": file}),
            "documentSymbolProvider",
        ),
        (
            "workspaceSymbol",
            json!({"query": "main"}),
            "workspaceSymbolProvider",
        ),
        (
            "codeAction",
            json!({"file": file, "line": 1, "column": 1}),
            "codeActionProvider",
        ),
        (
            "applyCodeAction",
            json!({"file": file, "line": 1, "column": 1, "index": 0}),
            "codeActionProvider",
        ),
        (
            "rename",
            json!({"file": file, "line": 1, "column": 1, "new_name": "x"}),
            "renameProvider",
        ),
        (
            "format",
            json!({"file": file}),
            "documentFormattingProvider",
        ),
        (
            "format",
            json!({
                "file": file,
                "range": {"line": 1, "column": 1, "end_line": 2, "end_column": 1}
            }),
            "documentRangeFormattingProvider",
        ),
    ];
    for (name, arguments, capability) in cases {
        let result = client.call(name, arguments);
        assert_eq!(
            result["isError"],
            json!(true),
            "{name} must be an error: {result}"
        );
        let text = result["content"][0]["text"].as_str().unwrap_or("");
        assert!(
            text.contains(capability),
            "`{name}` must name the missing capability `{capability}`: {text:?}"
        );
    }

    let (code, stderr) = client.finish();
    assert_eq!(code, 0, "expected a clean exit; stderr:\n{stderr}");
    assert!(
        !stderr.contains("panicked"),
        "the server must never panic:\n{stderr}"
    );

    let _ = std::fs::remove_file(&path);
}
