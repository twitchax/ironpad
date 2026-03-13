mod daemon;
mod ipc;

use std::io::Read;

use clap::{Parser, Subcommand, ValueEnum};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use crate::ipc::{IpcRequest, IpcResponse};

// ── CLI args ────────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(name = "ironpad-cli", about = "CLI for ironpad agent collaboration")]
struct Cli {
    /// Ironpad server URL (e.g. ws://localhost:3111)
    #[arg(long, env = "IRONPAD_HOST")]
    host: Option<String>,

    /// Session token
    #[arg(long, env = "IRONPAD_TOKEN")]
    token: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start the daemon process (normally auto-started).
    Daemon,
    /// Stop the running daemon.
    DaemonStop,
    /// Show daemon/connection status.
    Status,

    // ── Notebook commands ────────────────────────────────────────────────
    /// Get notebook metadata.
    Notebook,

    // ── Cell commands ────────────────────────────────────────────────────
    /// Cell operations.
    #[command(subcommand)]
    Cells(CellsCommand),

    /// Send a raw IPC command (for debugging).
    #[command(hide = true)]
    Raw {
        /// Command name.
        command: String,
        /// JSON args.
        #[arg(default_value = "{}")]
        args: String,
    },
}

#[derive(Clone, ValueEnum)]
enum CellTypeArg {
    Code,
    Markdown,
}

impl CellTypeArg {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Code => "code",
            Self::Markdown => "markdown",
        }
    }
}

#[derive(Subcommand)]
enum CellsCommand {
    /// List all cells in order.
    List,
    /// Get full cell content.
    Get {
        /// Cell ID.
        cell_id: String,
    },
    /// Add a new cell.
    Add {
        /// Cell source code. Use "-" to read from stdin.
        #[arg(long, conflicts_with = "source_file")]
        source: Option<String>,
        /// Read source from a file.
        #[arg(long, conflicts_with = "source")]
        source_file: Option<String>,
        /// Cell type.
        #[arg(long, default_value = "code")]
        r#type: CellTypeArg,
        /// Cell label.
        #[arg(long)]
        label: Option<String>,
        /// Insert after this cell ID. Omit to insert at beginning.
        #[arg(long)]
        after: Option<String>,
        /// Custom Cargo.toml content.
        #[arg(long)]
        cargo_toml: Option<String>,
    },
    /// Update a cell's source or metadata.
    Update {
        /// Cell ID.
        cell_id: String,
        /// New source code. Use "-" to read from stdin.
        #[arg(long, conflicts_with = "source_file")]
        source: Option<String>,
        /// Read source from a file.
        #[arg(long, conflicts_with = "source")]
        source_file: Option<String>,
        /// Update Cargo.toml content.
        #[arg(long)]
        cargo_toml: Option<String>,
        /// Update label.
        #[arg(long)]
        label: Option<String>,
        /// Expected version for OCC. Auto-fetched from daemon if omitted.
        #[arg(long)]
        version: Option<u64>,
    },
    /// Delete a cell.
    Delete {
        /// Cell ID.
        cell_id: String,
        /// Expected version. Auto-fetched from daemon if omitted.
        #[arg(long)]
        version: Option<u64>,
    },
    /// Set cell order. Provide all cell IDs in desired order.
    Reorder {
        /// Cell IDs in desired order.
        cell_ids: Vec<String>,
    },
}

// ── Main ────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    match cli.command {
        Command::Daemon => {
            tracing_subscriber::fmt()
                .with_env_filter(
                    tracing_subscriber::EnvFilter::try_from_default_env()
                        .unwrap_or_else(|_| "info".into()),
                )
                .init();

            let host = cli.host.unwrap_or_else(|| {
                eprintln!("error: --host or IRONPAD_HOST required for daemon mode");
                std::process::exit(1);
            });
            let token = cli.token.unwrap_or_else(|| {
                eprintln!("error: --token or IRONPAD_TOKEN required for daemon mode");
                std::process::exit(1);
            });

            if let Err(e) = daemon::run(&host, &token).await {
                eprintln!("daemon error: {e}");
                std::process::exit(1);
            }
        }

        Command::DaemonStop => {
            let pid_path = daemon::pid_path();
            match tokio::fs::read_to_string(&pid_path).await {
                Ok(pid_str) => {
                    if let Ok(pid) = pid_str.trim().parse::<u32>() {
                        match libc_kill(pid) {
                            Ok(()) => println!("sent stop signal to daemon (pid {pid})"),
                            Err(e) => {
                                eprintln!("failed to stop daemon: {e}");
                                std::process::exit(1);
                            }
                        }
                    } else {
                        eprintln!("invalid pidfile content");
                        std::process::exit(1);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    eprintln!("daemon is not running (no pidfile)");
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!("failed to read pidfile: {e}");
                    std::process::exit(1);
                }
            }
        }

        Command::Status => {
            let response = send_ipc("status", serde_json::Value::Null).await;
            print_response(&response);
        }

        Command::Notebook => {
            let response = send_ipc("notebook.get", serde_json::Value::Null).await;
            print_response(&response);
        }

        Command::Cells(cmd) => handle_cells_command(cmd).await,

        Command::Raw { command, args } => {
            let args: serde_json::Value = serde_json::from_str(&args).unwrap_or_default();
            let response = send_ipc(&command, args).await;
            print_response(&response);
        }
    }
}

// ── Cell command dispatch ───────────────────────────────────────────────────

async fn handle_cells_command(cmd: CellsCommand) {
    match cmd {
        CellsCommand::List => {
            let response = send_ipc("cells.list", serde_json::Value::Null).await;
            print_response(&response);
        }

        CellsCommand::Get { cell_id } => {
            let response = send_ipc("cells.get", serde_json::json!({ "cell_id": cell_id })).await;
            print_response(&response);
        }

        CellsCommand::Add {
            source,
            source_file,
            r#type,
            label,
            after,
            cargo_toml,
        } => {
            let source = resolve_source(source, source_file);
            let response = send_ipc(
                "cells.add",
                serde_json::json!({
                    "source": source.unwrap_or_default(),
                    "type": r#type.as_str(),
                    "label": label.unwrap_or_else(|| "New Cell".to_string()),
                    "after_cell_id": after,
                    "cargo_toml": cargo_toml,
                }),
            )
            .await;
            print_response(&response);
        }

        CellsCommand::Update {
            cell_id,
            source,
            source_file,
            cargo_toml,
            label,
            version,
        } => {
            let source = resolve_source(source, source_file);

            let version = match version {
                Some(v) => v,
                None => fetch_cell_version(&cell_id).await,
            };

            let mut args = serde_json::json!({
                "cell_id": cell_id,
                "version": version,
            });
            if let Some(src) = source {
                args["source"] = serde_json::Value::String(src);
            }
            if let Some(ct) = cargo_toml {
                args["cargo_toml"] = serde_json::Value::String(ct);
            }
            if let Some(lbl) = label {
                args["label"] = serde_json::Value::String(lbl);
            }

            let response = send_ipc("cells.update", args).await;
            print_response(&response);
        }

        CellsCommand::Delete { cell_id, version } => {
            let version = match version {
                Some(v) => v,
                None => fetch_cell_version(&cell_id).await,
            };

            let response = send_ipc(
                "cells.delete",
                serde_json::json!({
                    "cell_id": cell_id,
                    "version": version,
                }),
            )
            .await;
            print_response(&response);
        }

        CellsCommand::Reorder { cell_ids } => {
            let response =
                send_ipc("cells.reorder", serde_json::json!({ "cell_ids": cell_ids })).await;
            print_response(&response);
        }
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Fetch the current version of a cell from the daemon. Exits on error.
async fn fetch_cell_version(cell_id: &str) -> u64 {
    let resp = send_ipc("cells.get", serde_json::json!({ "cell_id": cell_id })).await;
    if !resp.ok {
        print_response(&resp); // exits with appropriate code
        unreachable!();
    }
    resp.data
        .as_ref()
        .and_then(|d| d.get("version"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0)
}

/// Resolve source from --source, --source-file, or stdin ("-").
fn resolve_source(source: Option<String>, source_file: Option<String>) -> Option<String> {
    if let Some(ref s) = source {
        if s == "-" {
            let mut buf = String::new();
            std::io::stdin()
                .read_to_string(&mut buf)
                .expect("failed to read stdin");
            return Some(buf);
        }
        return source;
    }
    if let Some(path) = source_file {
        return Some(std::fs::read_to_string(&path).unwrap_or_else(|e| {
            eprintln!("failed to read {path}: {e}");
            std::process::exit(1);
        }));
    }
    None
}

/// Print an IPC response as JSON to stdout. Exit with appropriate code on error.
fn print_response(response: &IpcResponse) {
    if response.ok {
        if let Some(data) = &response.data {
            println!(
                "{}",
                serde_json::to_string(data).expect("JSON serialization")
            );
        }
    } else {
        let error_json = serde_json::json!({
            "error": response.code.as_deref().unwrap_or("error"),
            "message": response.error.as_deref().unwrap_or("unknown error"),
        });
        eprintln!(
            "{}",
            serde_json::to_string(&error_json).expect("JSON serialization")
        );

        let exit_code = match response.code.as_deref() {
            Some("VersionConflict") => 2,
            Some("PermissionDenied") => 3,
            Some(c) if c.contains("connect") || c.contains("disconnect") => 4,
            _ => {
                if response
                    .error
                    .as_deref()
                    .is_some_and(|e| e.contains("daemon") || e.contains("socket"))
                {
                    4
                } else {
                    1
                }
            }
        };
        std::process::exit(exit_code);
    }
}

// ── IPC client ──────────────────────────────────────────────────────────────

/// Send a command to the daemon via Unix socket and return the response.
async fn send_ipc(command: &str, args: serde_json::Value) -> IpcResponse {
    let sock = daemon::socket_path();

    let stream = match UnixStream::connect(&sock).await {
        Ok(s) => s,
        Err(_) => {
            return IpcResponse::error_with_code(
                "daemon is not running (cannot connect to socket)",
                "connection_error",
            );
        }
    };

    let (reader, mut writer) = stream.into_split();

    let req = IpcRequest {
        command: command.to_string(),
        args,
    };

    let mut json = serde_json::to_string(&req).expect("IPC request serialization");
    json.push('\n');

    if writer.write_all(json.as_bytes()).await.is_err() {
        return IpcResponse::error("failed to send request to daemon");
    }

    let mut lines = BufReader::new(reader).lines();
    match lines.next_line().await {
        Ok(Some(line)) => serde_json::from_str(&line)
            .unwrap_or_else(|_| IpcResponse::error("invalid response from daemon")),
        _ => IpcResponse::error("no response from daemon"),
    }
}

// ── Signal helper ───────────────────────────────────────────────────────────

#[allow(unsafe_code)]
fn libc_kill(pid: u32) -> Result<(), String> {
    let pid = i32::try_from(pid).map_err(|_| format!("PID {pid} too large"))?;
    // SIGTERM = 15 on all Unix platforms.
    let ret = unsafe { libc::kill(pid, 15) };
    if ret == 0 {
        Ok(())
    } else {
        Err(format!("kill failed: {}", std::io::Error::last_os_error()))
    }
}
