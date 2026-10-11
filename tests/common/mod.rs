//! Shared helpers for the process-level integration tests.
//!
//! `tests/mcp.rs` (against the test-only `fake-lsp-server`) and
//! `tests/language_servers.rs` (against real language servers) both drive the
//! compiled `lsp-mcp` binary through this client, so it lives here rather than
//! being duplicated in each test crate. It is included with `mod common;` and is
//! not itself a test target.
//!
//! The client speaks **newline-delimited** JSON-RPC to the child's stdout and
//! asserts the adapter's contract along the way: every non-empty stdout line is
//! a JSON-RPC message, and the session ends with a clean exit once stdin closes.

#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// How long to wait for any single MCP message before failing the test.
pub const MESSAGE_TIMEOUT: Duration = Duration::from_secs(15);

/// A minimal MCP client over the child's stdio.
pub struct McpClient {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: Receiver<String>,
    stderr: JoinHandle<String>,
    next_id: i64,
}

impl McpClient {
    /// Spawn the adapter with the test-only `fake-lsp-server`, forwarding `extra`
    /// arguments to it after `--`.
    pub fn spawn(extra: &[&str]) -> Self {
        Self::spawn_with(env!("CARGO_BIN_EXE_fake-lsp-server"), extra, None)
    }

    /// Spawn `lsp-mcp <program> [-- <args>...]`, optionally with `cwd` as the
    /// child's working directory (which a real language server uses as its root,
    /// since the adapter sends no `rootUri`).
    pub fn spawn_with(program: &str, args: &[&str], cwd: Option<&Path>) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_lsp-mcp"));
        command.arg(program);
        if !args.is_empty() {
            command.arg("--").args(args);
        }
        if let Some(cwd) = cwd {
            command.current_dir(cwd);
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .expect("spawn the lsp-mcp binary under test");

        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");

        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(line) => {
                        if sender.send(line).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        let stderr = thread::spawn(move || {
            let mut text = String::new();
            let _ = BufReader::new(stderr).read_to_string(&mut text);
            text
        });

        Self {
            child,
            stdin: Some(stdin),
            stdout: receiver,
            stderr,
            next_id: 1,
        }
    }

    fn write(&mut self, message: &Value) {
        let mut line = serde_json::to_string(message).expect("encode a JSON-RPC message");
        line.push('\n');
        let stdin = self.stdin.as_mut().expect("stdin is open");
        stdin.write_all(line.as_bytes()).expect("write to stdin");
        stdin.flush().expect("flush stdin");
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.write(&json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    pub fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.write(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let message = self.read_message();
            if message.get("id").and_then(Value::as_i64) == Some(id) {
                return message;
            }
        }
    }

    /// Complete the MCP handshake.
    pub fn initialize(&mut self) {
        let initialize = self.request(
            "initialize",
            json!({
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "lsp-mcp-test", "version": "0.1.0"},
            }),
        );
        assert!(
            initialize.get("error").is_none(),
            "initialize failed: {initialize}"
        );
        assert_ne!(
            initialize["result"]["capabilities"]["tools"],
            json!(null),
            "the server must advertise the tools capability: {initialize}"
        );
        self.notify("notifications/initialized", json!({}));
    }

    /// Call a tool and return its `result` object.
    pub fn call(&mut self, name: &str, arguments: Value) -> Value {
        let call = self.request("tools/call", json!({"name": name, "arguments": arguments}));
        call["result"].clone()
    }

    /// Read the next non-empty stdout line, asserting it is a JSON-RPC message.
    fn read_message(&mut self) -> Value {
        loop {
            match self.stdout.recv_timeout(MESSAGE_TIMEOUT) {
                Ok(line) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    return serde_json::from_str(trimmed).unwrap_or_else(|error| {
                        panic!("every non-empty stdout line must be JSON, got {trimmed:?}: {error}")
                    });
                }
                Err(RecvTimeoutError::Timeout) => panic!("timed out waiting for an MCP message"),
                Err(RecvTimeoutError::Disconnected) => {
                    panic!("stdout closed before a message arrived")
                }
            }
        }
    }

    /// Close stdin (ending the session), wait for exit, and return `(code, stderr)`.
    pub fn finish(mut self) -> (i32, String) {
        self.stdin = None;

        let start = Instant::now();
        let status = loop {
            if let Some(status) = self.child.try_wait().expect("wait for the child") {
                break status;
            }
            if start.elapsed() > MESSAGE_TIMEOUT {
                let _ = self.child.kill();
                let _ = self.child.wait();
                panic!("lsp-mcp did not exit after stdin closed");
            }
            thread::sleep(Duration::from_millis(20));
        };

        // Drain any trailing stdout, still asserting it is JSON.
        while let Ok(line) = self.stdout.recv_timeout(Duration::from_secs(2)) {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                serde_json::from_str::<Value>(trimmed).unwrap_or_else(|error| {
                    panic!("trailing stdout was not JSON: {trimmed:?}: {error}")
                });
            }
        }

        let stderr = self.stderr.join().expect("join the stderr reader");
        (status.code().unwrap_or(-1), stderr)
    }
}

/// Write a small Rust source file with a unique name for `tag` and return it.
pub fn temp_source_path(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("lsp-mcp-{tag}-{}.rs", std::process::id()));
    std::fs::write(&path, "fn main() {\n    let answer = 42;\n}\n").expect("write the temp file");
    path
}

pub fn expected_uri(path: &std::path::Path) -> String {
    format!("file://{}", path.display())
}

/// The deterministic sibling path the fake server derives for a rename: the file
/// stem gains a `-sibling` suffix.
pub fn sibling_path(path: &std::path::Path) -> PathBuf {
    let text = path.to_string_lossy();
    let stem = text.strip_suffix(".rs").unwrap_or(&text);
    PathBuf::from(format!("{stem}-sibling.rs"))
}
