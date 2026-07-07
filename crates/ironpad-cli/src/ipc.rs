//! IPC protocol for communication between the CLI client and daemon.
//!
//! JSON-over-Unix-socket, newline-delimited. Each message is a single
//! JSON object followed by `\n`.

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufRead, AsyncBufReadExt};

/// Maximum size (in bytes) of a single newline-delimited IPC frame.
///
/// The wire is one JSON object per line. A client that opens the socket and
/// streams bytes without ever sending a `\n` would otherwise make the reader
/// buffer without bound (local OOM). 1 MiB dwarfs any legitimate request or
/// response, so this only ever trips on a misbehaving/abusive peer.
pub const MAX_IPC_FRAME: usize = 1024 * 1024;

/// Request from the CLI client to the daemon.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IpcRequest {
    /// Command name (e.g. "cells.list", "cells.update", "notebook.get").
    pub command: String,
    /// Command-specific arguments.
    #[serde(default)]
    pub args: serde_json::Value,
}

/// Response from the daemon to the CLI client.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IpcResponse {
    /// Whether the command succeeded.
    pub ok: bool,
    /// Response data (command-specific).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    /// Error message if `ok` is false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Error code if `ok` is false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

impl IpcResponse {
    pub fn success(data: serde_json::Value) -> Self {
        Self {
            ok: true,
            data: Some(data),
            error: None,
            code: None,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            data: None,
            error: Some(message.into()),
            code: None,
        }
    }

    pub fn error_with_code(message: impl Into<String>, code: impl Into<String>) -> Self {
        Self {
            ok: false,
            data: None,
            error: Some(message.into()),
            code: Some(code.into()),
        }
    }
}

// ── Framing ───────────────────────────────────────────────────────────────────

/// Read one newline-delimited frame from `reader`, enforcing [`MAX_IPC_FRAME`].
///
/// Returns `Ok(None)` at clean EOF, `Ok(Some(line))` for a complete frame (the
/// trailing `\n`/`\r\n` stripped, or a final unterminated frame at EOF), or an
/// `InvalidData` error if the frame exceeds the cap — so a peer that streams
/// bytes without a newline is disconnected instead of exhausting memory. This
/// replaces `AsyncBufReadExt::lines()`, whose per-line buffer is unbounded.
pub async fn read_frame<R>(reader: &mut R) -> std::io::Result<Option<String>>
where
    R: AsyncBufRead + Unpin,
{
    let mut buf: Vec<u8> = Vec::new();

    loop {
        let chunk = reader.fill_buf().await?;

        if chunk.is_empty() {
            // EOF. A non-empty buffer is a final, unterminated frame.
            if buf.is_empty() {
                return Ok(None);
            }
            break;
        }

        if let Some(pos) = chunk.iter().position(|&b| b == b'\n') {
            if buf.len() + pos > MAX_IPC_FRAME {
                return Err(frame_too_large());
            }
            buf.extend_from_slice(&chunk[..pos]);
            reader.consume(pos + 1); // discard the newline as well
            break;
        }

        // No newline in this chunk — accumulate it (bounded) and read on.
        if buf.len() + chunk.len() > MAX_IPC_FRAME {
            return Err(frame_too_large());
        }
        let consumed = chunk.len();
        buf.extend_from_slice(chunk);
        reader.consume(consumed);
    }

    // Mirror `lines()`: strip a trailing `\r` from a CRLF terminator.
    if buf.last() == Some(&b'\r') {
        buf.pop();
    }

    // The wire is UTF-8 JSON; lossy decoding is fine since a non-UTF-8 frame
    // won't parse as a request/response anyway (handled by the caller).
    Ok(Some(String::from_utf8_lossy(&buf).into_owned()))
}

fn frame_too_large() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "IPC frame exceeds maximum size",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_round_trips_through_json() {
        let req = IpcRequest {
            command: "cells.update".to_string(),
            args: json!({ "cell_id": "abc", "source": "let x = 1;" }),
        };
        let wire = serde_json::to_string(&req).unwrap();
        let back: IpcRequest = serde_json::from_str(&wire).unwrap();
        assert_eq!(back.command, "cells.update");
        assert_eq!(back.args["cell_id"], "abc");
        assert_eq!(back.args["source"], "let x = 1;");
    }

    #[test]
    fn request_defaults_missing_args_to_null() {
        // `args` is #[serde(default)] — a command with no args must still parse.
        let back: IpcRequest = serde_json::from_str(r#"{"command":"status"}"#).unwrap();
        assert_eq!(back.command, "status");
        assert!(back.args.is_null(), "missing args -> Null: {:?}", back.args);
    }

    #[test]
    fn newline_delimited_framing_round_trips() {
        // The wire contract (module doc): one JSON object per line. Two framed
        // requests must decode back independently when split on '\n'.
        let a = IpcRequest {
            command: "cells.list".to_string(),
            args: serde_json::Value::Null,
        };
        let b = IpcRequest {
            command: "notebook.get".to_string(),
            args: serde_json::Value::Null,
        };
        let mut frame = String::new();
        frame.push_str(&serde_json::to_string(&a).unwrap());
        frame.push('\n');
        frame.push_str(&serde_json::to_string(&b).unwrap());
        frame.push('\n');

        let mut lines = frame.lines();
        let da: IpcRequest = serde_json::from_str(lines.next().unwrap()).unwrap();
        let db: IpcRequest = serde_json::from_str(lines.next().unwrap()).unwrap();
        assert!(lines.next().is_none(), "exactly two frames");
        assert_eq!(da.command, "cells.list");
        assert_eq!(db.command, "notebook.get");
    }

    #[test]
    fn success_response_omits_error_fields() {
        let wire = serde_json::to_string(&IpcResponse::success(json!({ "version": 3 }))).unwrap();
        // skip_serializing_if keeps the wire minimal.
        assert!(wire.contains("\"ok\":true"), "{wire}");
        assert!(wire.contains("\"data\""), "{wire}");
        assert!(!wire.contains("\"error\""), "no error field: {wire}");
        assert!(!wire.contains("\"code\""), "no code field: {wire}");
    }

    #[test]
    fn error_response_omits_data() {
        let wire = serde_json::to_string(&IpcResponse::error("boom")).unwrap();
        assert!(wire.contains("\"ok\":false"), "{wire}");
        assert!(wire.contains("\"error\":\"boom\""), "{wire}");
        assert!(!wire.contains("\"data\""), "no data field: {wire}");
        assert!(!wire.contains("\"code\""), "no code field: {wire}");
    }

    #[test]
    fn error_with_code_round_trips() {
        let resp = IpcResponse::error_with_code("version conflict", "VERSION_CONFLICT");
        let back: IpcResponse =
            serde_json::from_str(&serde_json::to_string(&resp).unwrap()).unwrap();
        assert!(!back.ok);
        assert_eq!(back.error.as_deref(), Some("version conflict"));
        assert_eq!(back.code.as_deref(), Some("VERSION_CONFLICT"));
        assert!(back.data.is_none());
    }

    // ── read_frame (bounded, newline-delimited) ──────────────────────────

    #[tokio::test]
    async fn read_frame_returns_one_line_then_eof() {
        let data = b"{\"command\":\"status\"}\n";
        let mut r = tokio::io::BufReader::new(&data[..]);
        assert_eq!(
            read_frame(&mut r).await.unwrap().unwrap(),
            r#"{"command":"status"}"#
        );
        assert!(read_frame(&mut r).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn read_frame_reads_sequential_frames() {
        let data = b"first\nsecond\n";
        let mut r = tokio::io::BufReader::new(&data[..]);
        assert_eq!(read_frame(&mut r).await.unwrap().unwrap(), "first");
        assert_eq!(read_frame(&mut r).await.unwrap().unwrap(), "second");
        assert!(read_frame(&mut r).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn read_frame_strips_crlf_terminator() {
        let data = b"line\r\n";
        let mut r = tokio::io::BufReader::new(&data[..]);
        assert_eq!(read_frame(&mut r).await.unwrap().unwrap(), "line");
    }

    #[tokio::test]
    async fn read_frame_returns_trailing_unterminated_frame() {
        // No trailing newline: the final bytes are still delivered exactly once.
        let data = b"no-newline";
        let mut r = tokio::io::BufReader::new(&data[..]);
        assert_eq!(read_frame(&mut r).await.unwrap().unwrap(), "no-newline");
        assert!(read_frame(&mut r).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn read_frame_rejects_oversized_frame() {
        // A peer streaming past the cap with no newline is rejected, not
        // buffered without bound.
        let big = vec![b'x'; MAX_IPC_FRAME + 16];
        let mut r = tokio::io::BufReader::new(&big[..]);
        let err = read_frame(&mut r).await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[tokio::test]
    async fn read_frame_accepts_frame_at_limit() {
        let mut data = vec![b'x'; MAX_IPC_FRAME];
        data.push(b'\n');
        let mut r = tokio::io::BufReader::new(&data[..]);
        let frame = read_frame(&mut r).await.unwrap().unwrap();
        assert_eq!(frame.len(), MAX_IPC_FRAME);
    }
}
