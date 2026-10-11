//! Process-level tests for the compiled `lsp-mcp` binary.
//!
//! These spawn the real executable (and the `fake-lsp-server` helper binary)
//! through Cargo's `CARGO_BIN_EXE_<name>` environment variables, so they cover
//! launching the server by name, the handshake, the stdout-only-MCP contract,
//! and the clean non-zero exit for a missing server — with no network or
//! external server.
//! The server now serves MCP over stdio, so the harness drives a minimal
//! `initialize`/`initialized` session and closes stdin to end it.

use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Run a command to completion, returning `(exit code, stdout, stderr)`.
///
/// When `stdin_data` is `Some`, it is written to the child's stdin (which is then
/// closed, ending the session); otherwise stdin is `/dev/null`. The child is
/// killed if it outlives `timeout`, so a regression fails the test instead of
/// hanging it.
fn run(mut command: Command, stdin_data: Option<&str>, timeout: Duration) -> (i32, String, String) {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    if stdin_data.is_some() {
        command.stdin(Stdio::piped());
    } else {
        command.stdin(Stdio::null());
    }
    let mut child: Child = command.spawn().expect("spawn the binary under test");
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");

    if let Some(data) = stdin_data {
        let mut stdin = child.stdin.take().expect("piped stdin");
        stdin.write_all(data.as_bytes()).expect("write stdin");
        // Dropping `stdin` closes it, which ends the MCP session.
    }

    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().expect("wait for the child") {
            let mut out = String::new();
            let mut err = String::new();
            stdout.read_to_string(&mut out).expect("read stdout");
            stderr.read_to_string(&mut err).expect("read stderr");
            return (status.code().unwrap_or(-1), out, err);
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the child did not exit within {timeout:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn lsp_mcp() -> Command {
    Command::new(env!("CARGO_BIN_EXE_lsp-mcp"))
}

/// A minimal newline-delimited MCP session: `initialize` then `initialized`.
fn initialize_session() -> String {
    [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"cli-test","version":"0.1.0"}}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}"#,
    ]
    .join("\n")
        + "\n"
}

/// Assert every non-empty stdout line is a JSON-RPC message.
fn assert_stdout_is_mcp(stdout: &str) {
    for line in stdout.lines() {
        if line.trim().is_empty() {
            continue;
        }
        serde_json::from_str::<serde_json::Value>(line)
            .unwrap_or_else(|error| panic!("stdout line was not JSON: {line:?}: {error}"));
    }
}

#[test]
fn passthrough_launches_the_server_and_serves_mcp() {
    let mut command = lsp_mcp();
    command.arg(env!("CARGO_BIN_EXE_fake-lsp-server"));

    let (code, stdout, stderr) = run(
        command,
        Some(&initialize_session()),
        Duration::from_secs(20),
    );

    assert_eq!(code, 0, "expected a clean exit; stderr was:\n{stderr}");
    assert_stdout_is_mcp(&stdout);
    assert!(
        stdout.contains("serverInfo"),
        "expected the initialize response on stdout, got:\n{stdout}"
    );
    assert!(
        stderr.contains("capabilities captured"),
        "expected the handshake summary on stderr, got:\n{stderr}"
    );
}

#[test]
fn a_missing_server_exits_nonzero_with_a_clean_message() {
    let mut command = lsp_mcp();
    command.arg("/definitely/not/a/real/lsp-server-binary-lsp-mcp");

    let (code, stdout, stderr) = run(command, None, Duration::from_secs(20));

    assert_ne!(code, 0, "a missing server must not exit zero");
    assert_eq!(stdout, "", "stdout must stay empty on failure too");
    assert!(
        stderr.contains("failed to start LSP server"),
        "expected a clean error message, got:\n{stderr}"
    );
    assert!(
        !stderr.contains("panicked"),
        "the failure must not be a panic, got:\n{stderr}"
    );
}

#[test]
fn no_server_argument_is_a_clean_usage_error() {
    let (code, stdout, stderr) = run(lsp_mcp(), None, Duration::from_secs(20));

    assert_ne!(code, 0, "a missing server argument must not exit zero");
    assert_eq!(stdout, "", "usage errors must not write to stdout");
    assert!(
        stderr.to_lowercase().contains("server"),
        "expected a usage message naming the missing argument, got:\n{stderr}"
    );
    assert!(!stderr.contains("panicked"), "got:\n{stderr}");
}
