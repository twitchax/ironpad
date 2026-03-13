//! IPC protocol for communication between the CLI client and daemon.
//!
//! JSON-over-Unix-socket, newline-delimited. Each message is a single
//! JSON object followed by `\n`.

use serde::{Deserialize, Serialize};

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
