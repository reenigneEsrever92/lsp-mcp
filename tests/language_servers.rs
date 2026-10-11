//! End-to-end tests against **real** language servers, driven through the sample
//! projects under `test-projects/`.
//!
//! Each test is gated on its language server (and, where needed, its toolchain)
//! being installed: when the binary is not found on `PATH` — or a project's
//! dependencies have not been installed — the test prints a `skipping ...` line to
//! stderr and returns, so `cargo test` stays green on a machine with no servers.
//! When the server *is* present it spawns the real `lsp-mcp` with the project as
//! its working directory and drives it over MCP exactly as an agent would.
//!
//! A real server loads the project and computes semantic results in the
//! background, so anything that needs more than the open file (cross-file
//! definitions and references, workspace symbols, compiler diagnostics) is
//! polled with [`retry_call`] until it settles or [`ANALYSIS_TIMEOUT`] elapses.
//!
//! Run just these with `cargo test --test language_servers`.

mod common;

use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use common::McpClient;

/// How long to keep polling for a result the server computes in the background
/// (for example `rust-analyzer` running `cargo check`) before giving up.
const ANALYSIS_TIMEOUT: Duration = Duration::from_secs(60);

/// The absolute path of the sample project named `name`.
fn project(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("test-projects")
        .join(name)
}

/// Locate `program` on `PATH`, applying the platform executable suffix.
fn which(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let exe = format!("{program}{}", std::env::consts::EXE_SUFFIX);
    std::env::split_paths(&path)
        .map(|dir| dir.join(&exe))
        .find(|candidate| candidate.is_file())
}

/// The path to `program`, or `None` after printing a skip notice.
fn server_or_skip(program: &str, test: &str) -> Option<String> {
    match which(program) {
        Some(path) => Some(path.to_string_lossy().into_owned()),
        None => {
            eprintln!("skipping `{test}`: `{program}` is not on PATH");
            None
        }
    }
}

fn skip(test: &str, reason: &str) {
    eprintln!("skipping `{test}`: {reason}");
}

/// The `name` of every symbol in a `documentSymbol`/`workspaceSymbol` result.
fn symbol_names(result: &Value) -> Vec<String> {
    result["structuredContent"]["symbols"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|symbol| symbol["name"].as_str().map(str::to_owned))
        .collect()
}

/// The lower-cased `message` of every diagnostic in a `diagnostics` result.
fn diagnostic_messages(result: &Value) -> Vec<String> {
    result["structuredContent"]["diagnostics"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|diagnostic| diagnostic["message"].as_str().map(str::to_lowercase))
        .collect()
}

/// The URI of every location in a `definition`/`references` result.
fn location_uris(result: &Value) -> Vec<String> {
    result["structuredContent"]["locations"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|location| location["uri"].as_str().map(str::to_owned))
        .collect()
}

/// The rendered `hover` contents, if any.
fn hover_contents(result: &Value) -> String {
    result["structuredContent"]["hover"]["contents"]
        .as_str()
        .unwrap_or("")
        .to_owned()
}

/// Call `tool` repeatedly until `predicate` holds for its (non-error) result,
/// panicking with `what` if [`ANALYSIS_TIMEOUT`] elapses first. This absorbs the
/// asynchronous project loading and background analysis of a real language server.
fn retry_call(
    client: &mut McpClient,
    tool: &str,
    arguments: Value,
    predicate: impl Fn(&Value) -> bool,
    what: &str,
) -> Value {
    let deadline = Instant::now() + ANALYSIS_TIMEOUT;
    loop {
        let result = client.call(tool, arguments.clone());
        // A server still loading the project may answer with a transient error
        // (rust-analyzer's `ContentModified`, -32801); treat any error as "not
        // ready yet" and keep polling rather than failing outright.
        let errored = result["isError"].as_bool().unwrap_or(false);
        if !errored && predicate(&result) {
            return result;
        }
        if Instant::now() >= deadline {
            panic!("{what}: not satisfied within {ANALYSIS_TIMEOUT:?}: {result}");
        }
        thread::sleep(Duration::from_millis(500));
    }
}

/// Assert the MCP session ended cleanly and the adapter never panicked.
fn assert_clean_exit(client: McpClient, server: &str) {
    let (code, stderr) = client.finish();
    assert_eq!(
        code, 0,
        "{server}: expected a clean exit; stderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("panicked"),
        "{server}: the adapter must never panic:\n{stderr}"
    );
}

/// Poll `diagnostics` for `file` until a message contains `needle`
/// (case-insensitive).
fn diagnostics_containing(client: &mut McpClient, file: &Path, needle: &str) -> Value {
    let arguments = json!({"file": file.to_string_lossy()});
    let needle = needle.to_lowercase();
    retry_call(
        client,
        "diagnostics",
        arguments,
        |result| {
            diagnostic_messages(result)
                .iter()
                .any(|m| m.contains(&needle))
        },
        &format!("a diagnostic containing {needle:?}"),
    )
}

/// The `test-projects/` fixtures the language tests depend on must exist.
#[test]
fn test_projects_are_present() {
    let expected: &[(&str, &[&str])] = &[
        (
            "c-cpp",
            &["include/greeter.hpp", "src/greeter.cpp", "src/main.cpp"],
        ),
        (
            "typescript",
            &[
                "package.json",
                "tsconfig.json",
                "src/greeter.ts",
                "src/index.ts",
            ],
        ),
        ("rust", &["Cargo.toml", "src/lib.rs", "src/main.rs"]),
        ("go", &["go.mod", "main.go", "greeter/greeter.go"]),
    ];
    for (name, files) in expected {
        let root = project(name);
        assert!(
            root.is_dir(),
            "test project `{name}` is missing at {}",
            root.display()
        );
        for file in *files {
            let path = root.join(file);
            assert!(
                path.is_file(),
                "test project `{name}` is missing `{file}` at {}",
                path.display()
            );
        }
    }
}

#[test]
fn rust_analyzer_serves_the_rust_project() {
    let test = "rust_analyzer_serves_the_rust_project";
    let Some(server) = server_or_skip("rust-analyzer", test) else {
        return;
    };
    let root = project("rust");
    let main = root.join("src/main.rs");
    let lib = root.join("src/lib.rs");

    let mut client = McpClient::spawn_with(&server, &[], Some(&root));
    client.initialize();

    // documentSymbol decodes rust-analyzer's hierarchical response.
    let result = client.call("documentSymbol", json!({"file": lib.to_string_lossy()}));
    assert_eq!(result["isError"], json!(false), "documentSymbol: {result}");
    let names = symbol_names(&result);
    assert!(
        names.iter().any(|name| name == "Person"),
        "expected `Person` among {names:?}: {result}"
    );
    assert!(
        names.iter().any(|name| name == "greet"),
        "expected `greet` among {names:?}: {result}"
    );

    // definition crosses files: `Person` in main.rs resolves to lib.rs.
    retry_call(
        &mut client,
        "definition",
        json!({"file": main.to_string_lossy(), "line": 4, "column": 18}),
        |result| {
            location_uris(result)
                .iter()
                .any(|uri| uri.ends_with("/src/lib.rs"))
        },
        "definition of `Person` must reach lib.rs",
    );

    // references span both files.
    retry_call(
        &mut client,
        "references",
        json!({
            "file": main.to_string_lossy(),
            "line": 4,
            "column": 18,
            "include_declaration": true
        }),
        |result| {
            let uris = location_uris(result);
            uris.iter().any(|uri| uri.ends_with("/src/lib.rs"))
                && uris.iter().any(|uri| uri.ends_with("/src/main.rs"))
        },
        "references to `Person` must span lib.rs and main.rs",
    );

    // hover renders the `greet` documentation.
    retry_call(
        &mut client,
        "hover",
        json!({"file": main.to_string_lossy(), "line": 6, "column": 20}),
        |result| hover_contents(result).contains("greet"),
        "hover on `greet` must render it",
    );

    // workspaceSymbol finds the struct by query. (rust-analyzer's fuzzy search
    // matches the query `greet` to the crate named `greeter`, not the function,
    // so query a name that unambiguously resolves.)
    retry_call(
        &mut client,
        "workspaceSymbol",
        json!({"query": "Person"}),
        |result| symbol_names(result).iter().any(|name| name == "Person"),
        "workspaceSymbol must find `Person`",
    );

    // diagnostics reports the deliberate unused variable (via `cargo check`).
    diagnostics_containing(&mut client, &main, "unused");

    assert_clean_exit(client, "rust-analyzer");
}

#[test]
fn typescript_language_server_serves_the_typescript_project() {
    let test = "typescript_language_server_serves_the_typescript_project";
    let Some(server) = server_or_skip("typescript-language-server", test) else {
        return;
    };
    let root = project("typescript");
    if !root.join("node_modules").is_dir() {
        skip(test, "run `npm install` in test-projects/typescript first");
        return;
    }
    let index = root.join("src/index.ts");
    let greeter = root.join("src/greeter.ts");

    let mut client = McpClient::spawn_with(&server, &["--stdio"], Some(&root));
    client.initialize();

    let result = client.call("documentSymbol", json!({"file": greeter.to_string_lossy()}));
    assert_eq!(result["isError"], json!(false), "documentSymbol: {result}");
    let names = symbol_names(&result);
    assert!(
        names.iter().any(|name| name == "Person"),
        "expected `Person` among {names:?}: {result}"
    );
    assert!(
        names.iter().any(|name| name == "greet"),
        "expected `greet` among {names:?}: {result}"
    );

    // definition crosses files: `person` in index.ts resolves to greeter.ts.
    retry_call(
        &mut client,
        "definition",
        json!({"file": index.to_string_lossy(), "line": 4, "column": 13}),
        |result| {
            location_uris(result)
                .iter()
                .any(|uri| uri.ends_with("/src/greeter.ts"))
        },
        "definition of `person` must reach greeter.ts",
    );

    // hover renders the `greet` signature.
    retry_call(
        &mut client,
        "hover",
        json!({"file": index.to_string_lossy(), "line": 6, "column": 13}),
        |result| hover_contents(result).contains("greet"),
        "hover on `greet` must render it",
    );

    // workspaceSymbol finds the exported function.
    retry_call(
        &mut client,
        "workspaceSymbol",
        json!({"query": "greet"}),
        |result| {
            symbol_names(result)
                .iter()
                .any(|name| name.to_lowercase().contains("greet"))
        },
        "workspaceSymbol must find `greet`",
    );

    // diagnostics reports the deliberate unused binding (`noUnusedLocals`).
    diagnostics_containing(&mut client, &index, "unused");

    assert_clean_exit(client, "typescript-language-server");
}

#[test]
fn gopls_serves_the_go_project() {
    let test = "gopls_serves_the_go_project";
    let Some(server) = server_or_skip("gopls", test) else {
        return;
    };
    if which("go").is_none() {
        skip(
            test,
            "`gopls` needs the `go` toolchain, which is not on PATH",
        );
        return;
    }
    let root = project("go");
    let main = root.join("main.go");
    let greeter = root.join("greeter/greeter.go");

    let mut client = McpClient::spawn_with(&server, &[], Some(&root));
    client.initialize();

    let result = client.call("documentSymbol", json!({"file": greeter.to_string_lossy()}));
    assert_eq!(result["isError"], json!(false), "documentSymbol: {result}");
    let names = symbol_names(&result);
    assert!(
        names.iter().any(|name| name == "Person"),
        "expected `Person` among {names:?}: {result}"
    );
    assert!(
        names.iter().any(|name| name == "Greet"),
        "expected `Greet` among {names:?}: {result}"
    );

    // definition crosses packages: `greeter` in main.go resolves to greeter.go.
    retry_call(
        &mut client,
        "definition",
        json!({"file": main.to_string_lossy(), "line": 10, "column": 12}),
        |result| {
            location_uris(result)
                .iter()
                .any(|uri| uri.ends_with("/greeter/greeter.go"))
        },
        "definition of `greeter` must reach greeter.go",
    );

    // hover renders the `Greet` signature.
    retry_call(
        &mut client,
        "hover",
        json!({"file": main.to_string_lossy(), "line": 12, "column": 22}),
        |result| hover_contents(result).contains("Greet"),
        "hover on `Greet` must render it",
    );

    // workspaceSymbol finds the function.
    retry_call(
        &mut client,
        "workspaceSymbol",
        json!({"query": "Greet"}),
        |result| {
            symbol_names(result)
                .iter()
                .any(|name| name.to_lowercase().contains("greet"))
        },
        "workspaceSymbol must find `Greet`",
    );

    // diagnostics reports the deliberate unused variable (a compile error in Go).
    diagnostics_containing(&mut client, &main, "unused");

    assert_clean_exit(client, "gopls");
}

#[test]
fn clangd_serves_the_c_cpp_project() {
    let test = "clangd_serves_the_c_cpp_project";
    let Some(server) = server_or_skip("clangd", test) else {
        return;
    };
    let root = project("c-cpp");
    let main = root.join("src/main.cpp");
    let header = root.join("include/greeter.hpp");

    let mut client = McpClient::spawn_with(&server, &[], Some(&root));
    client.initialize();

    let result = client.call("documentSymbol", json!({"file": header.to_string_lossy()}));
    assert_eq!(result["isError"], json!(false), "documentSymbol: {result}");
    let names = symbol_names(&result);
    assert!(
        names.iter().any(|name| name == "Person"),
        "expected `Person` among {names:?}: {result}"
    );
    assert!(
        names.iter().any(|name| name == "greet"),
        "expected `greet` among {names:?}: {result}"
    );

    // definition crosses files: the `greeter` namespace in main.cpp resolves to
    // the header, proving `.clangd` supplied the include path.
    retry_call(
        &mut client,
        "definition",
        json!({"file": main.to_string_lossy(), "line": 6, "column": 11}),
        |result| {
            location_uris(result)
                .iter()
                .any(|uri| uri.ends_with("/include/greeter.hpp"))
        },
        "definition of `greeter` must reach greeter.hpp",
    );

    // references find the struct's uses across the project.
    retry_call(
        &mut client,
        "references",
        json!({
            "file": main.to_string_lossy(),
            "line": 6,
            "column": 17,
            "include_declaration": true
        }),
        |result| !location_uris(result).is_empty(),
        "references must find `Person`",
    );

    // hover renders the overload's signature.
    retry_call(
        &mut client,
        "hover",
        json!({"file": main.to_string_lossy(), "line": 8, "column": 27}),
        |result| hover_contents(result).contains("greet"),
        "hover on `greet` must render it",
    );

    // diagnostics reports the deliberate unused variable (`-Wunused-variable`).
    diagnostics_containing(&mut client, &main, "unused");

    assert_clean_exit(client, "clangd");
}
