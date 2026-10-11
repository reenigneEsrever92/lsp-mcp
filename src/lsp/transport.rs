//! `Content-Length` framing over any `AsyncRead`/`AsyncWrite`, plus the
//! `tokio::process` spawn constructor.
//!
//! The framing layer is deliberately generic so the same code drives a spawned
//! child's stdio and an in-process `tokio::io::duplex` pair in tests.

use std::process::Stdio;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::process::{Child, Command};

use crate::error::{Error, Result};

/// The largest header block the reader will assemble before refusing it.
const MAX_HEADER_LEN: usize = 8 * 1024;

/// The largest body the reader will accept, guarding against a hostile or
/// corrupt `Content-Length`.
const MAX_CONTENT_LENGTH: usize = 64 * 1024 * 1024;

/// Spawn an LSP server with piped stdin/stdout.
///
/// The child's stdout and stdin are wired into the same [`read_frame`] /
/// [`write_frame`] framing used everywhere else. `kill_on_drop` is set so the
/// server does not outlive its [`Child`] handle, and stderr is inherited so
/// server diagnostics land alongside the adapter's own logs on stderr rather
/// than on stdout.
pub fn spawn(command: &str, args: &[String]) -> Result<Child> {
    let mut cmd = Command::new(command);
    cmd.args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true);
    cmd.spawn().map_err(|source| Error::SpawnFailed {
        command: command.to_string(),
        source,
    })
}

/// Read one framed message body.
///
/// Returns `Ok(None)` on a clean EOF at a message boundary. Headers are read one
/// byte at a time, so the result is correct no matter how the underlying stream
/// chunks the data.
pub async fn read_frame<R>(reader: &mut R) -> Result<Option<Vec<u8>>>
where
    R: AsyncRead + Unpin,
{
    let mut header = Vec::with_capacity(64);
    let mut byte = [0u8; 1];
    loop {
        let read = reader.read(&mut byte).await?;
        if read == 0 {
            if header.is_empty() {
                return Ok(None);
            }
            return Err(Error::Framing(
                "unexpected EOF while reading message headers".to_string(),
            ));
        }
        header.push(byte[0]);
        if header.ends_with(b"\r\n\r\n") {
            break;
        }
        if header.len() > MAX_HEADER_LEN {
            return Err(Error::Framing(format!(
                "message headers exceed {MAX_HEADER_LEN} bytes"
            )));
        }
    }

    let header_text = std::str::from_utf8(&header)
        .map_err(|_| Error::Framing("message headers are not valid UTF-8".to_string()))?;
    let content_length = parse_content_length(header_text)?;

    if content_length > MAX_CONTENT_LENGTH {
        return Err(Error::Framing(format!(
            "Content-Length {content_length} exceeds the maximum of {MAX_CONTENT_LENGTH} bytes"
        )));
    }

    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body).await.map_err(|err| {
        if err.kind() == std::io::ErrorKind::UnexpectedEof {
            Error::Framing(format!(
                "unexpected EOF while reading a {content_length}-byte body"
            ))
        } else {
            Error::Io(err)
        }
    })?;
    Ok(Some(body))
}

/// Write one framed message body, followed by a flush.
pub async fn write_frame<W>(writer: &mut W, body: &[u8]) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    writer.write_all(header.as_bytes()).await?;
    writer.write_all(body).await?;
    writer.flush().await?;
    Ok(())
}

fn parse_content_length(headers: &str) -> Result<usize> {
    let mut value = None;
    for line in headers.split("\r\n") {
        if line.is_empty() {
            continue;
        }
        let (name, raw) = line
            .split_once(':')
            .ok_or_else(|| Error::Framing(format!("malformed header line: {line:?}")))?;
        if name.trim().eq_ignore_ascii_case("content-length") {
            let raw = raw.trim();
            let parsed: usize = raw
                .parse()
                .map_err(|_| Error::Framing(format!("invalid Content-Length value: {raw:?}")))?;
            value = Some(parsed);
        }
    }
    value.ok_or_else(|| Error::Framing("missing Content-Length header".to_string()))
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    use serde_json::{json, Value};
    use tokio::io::{AsyncRead, ReadBuf};

    use super::*;

    /// An in-memory reader that hands out at most `chunk` bytes per poll, so
    /// tests can exercise byte-at-a-time headers and split bodies deterministically.
    struct Trickle {
        data: Vec<u8>,
        pos: usize,
        chunk: usize,
    }

    impl Trickle {
        fn new(data: Vec<u8>, chunk: usize) -> Self {
            Trickle {
                data,
                pos: 0,
                chunk,
            }
        }
    }

    impl AsyncRead for Trickle {
        fn poll_read(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            let this = self.get_mut();
            if this.pos >= this.data.len() {
                return Poll::Ready(Ok(()));
            }
            let n = this
                .chunk
                .min(buf.remaining())
                .min(this.data.len() - this.pos);
            buf.put_slice(&this.data[this.pos..this.pos + n]);
            this.pos += n;
            Poll::Ready(Ok(()))
        }
    }

    fn frame(body: &str) -> Vec<u8> {
        let mut bytes = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
        bytes.extend_from_slice(body.as_bytes());
        bytes
    }

    #[tokio::test]
    async fn reads_a_normal_frame() {
        let bytes = frame(r#"{"jsonrpc":"2.0"}"#);
        let mut reader = &bytes[..];
        let body = read_frame(&mut reader).await.unwrap().unwrap();
        assert_eq!(body, br#"{"jsonrpc":"2.0"}"#);
        assert!(read_frame(&mut reader).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn reads_headers_delivered_one_byte_at_a_time() {
        let bytes = frame(r#"{"id":1}"#);
        let mut reader = Trickle::new(bytes, 1);
        let body = read_frame(&mut reader).await.unwrap().unwrap();
        assert_eq!(body, br#"{"id":1}"#);
    }

    #[tokio::test]
    async fn reads_a_body_split_across_reads() {
        let bytes = frame(r#"{"hello":"world"}"#);
        // Header bytes plus a partial body per poll.
        let mut reader = Trickle::new(bytes, 5);
        let body = read_frame(&mut reader).await.unwrap().unwrap();
        assert_eq!(body, br#"{"hello":"world"}"#);
    }

    #[tokio::test]
    async fn reads_two_frames_back_to_back() {
        let mut bytes = frame(r#"{"a":1}"#);
        bytes.extend_from_slice(&frame(r#"{"b":2}"#));
        let mut reader = Trickle::new(bytes, 3);
        assert_eq!(
            read_frame(&mut reader).await.unwrap().unwrap(),
            br#"{"a":1}"#
        );
        assert_eq!(
            read_frame(&mut reader).await.unwrap().unwrap(),
            br#"{"b":2}"#
        );
        assert!(read_frame(&mut reader).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn missing_content_length_is_a_framing_error() {
        let bytes = b"X-Header: 1\r\n\r\n{}".to_vec();
        let mut reader = &bytes[..];
        let err = read_frame(&mut reader).await.unwrap_err();
        assert!(matches!(err, Error::Framing(ref m) if m.contains("missing Content-Length")));
    }

    #[tokio::test]
    async fn malformed_content_length_is_a_framing_error() {
        let bytes = b"Content-Length: banana\r\n\r\n".to_vec();
        let mut reader = &bytes[..];
        let err = read_frame(&mut reader).await.unwrap_err();
        assert!(matches!(err, Error::Framing(ref m) if m.contains("invalid Content-Length")));
    }

    #[tokio::test]
    async fn oversized_content_length_is_a_framing_error() {
        let bytes = format!("Content-Length: {}\r\n\r\n", MAX_CONTENT_LENGTH + 1).into_bytes();
        let mut reader = &bytes[..];
        let err = read_frame(&mut reader).await.unwrap_err();
        assert!(matches!(err, Error::Framing(ref m) if m.contains("exceeds the maximum")));
    }

    #[tokio::test]
    async fn eof_mid_header_is_a_framing_error() {
        let bytes = b"Content-Length: 5\r\n".to_vec();
        let mut reader = &bytes[..];
        let err = read_frame(&mut reader).await.unwrap_err();
        assert!(matches!(err, Error::Framing(ref m) if m.contains("unexpected EOF")));
    }

    #[tokio::test]
    async fn eof_mid_body_is_a_framing_error() {
        let bytes = b"Content-Length: 10\r\n\r\nshort".to_vec();
        let mut reader = &bytes[..];
        let err = read_frame(&mut reader).await.unwrap_err();
        assert!(matches!(err, Error::Framing(ref m) if m.contains("unexpected EOF")));
    }

    #[tokio::test]
    async fn writes_a_frame() {
        let mut buffer: Vec<u8> = Vec::new();
        write_frame(&mut buffer, br#"{"ok":true}"#).await.unwrap();
        assert_eq!(buffer, frame(r#"{"ok":true}"#));
        // And it round-trips back through the reader.
        let mut reader = &buffer[..];
        let body = read_frame(&mut reader).await.unwrap().unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value, json!({"ok": true}));
    }

    #[test]
    fn unlaunchable_server_is_a_typed_spawn_error() {
        let err = spawn(
            "definitely-not-a-real-lsp-binary-lsp-mcp-test",
            &["--stdio".to_string()],
        )
        .unwrap_err();
        match err {
            Error::SpawnFailed { command, .. } => {
                assert_eq!(command, "definitely-not-a-real-lsp-binary-lsp-mcp-test");
            }
            other => panic!("expected SpawnFailed, got {other:?}"),
        }
    }
}
