//! ironpad CLI: agent-facing commands and the session daemon.
//!
//! Subcommands translate to notebook mutations/queries routed over a Unix
//! socket to a long-lived [`daemon`] that holds the warm WebSocket to the
//! server. See [`daemon`] for the connection/IPC loop and [`ipc`] for the wire
//! framing.

mod daemon;
mod ipc;

use std::io::Read;

use clap::{Parser, Subcommand, ValueEnum};
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use crate::ipc::{IpcRequest, IpcResponse};

// ── CLI args ────────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(name = "ironpad-cli", about = "CLI for ironpad agent collaboration")]
struct Cli {
    /// Ironpad server URL (e.g. `ws://localhost:3111`)
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
        /// Create as a shared cell (source rides in every cell's shared.rs).
        #[arg(long)]
        shared: bool,
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
        /// Set the shared-cell flag: --shared true|false.
        #[arg(long)]
        shared: Option<bool>,
        /// Set whether the code body loads collapsed: --collapsed true|false.
        #[arg(long)]
        collapsed: Option<bool>,
        /// Set whether the output panel loads collapsed: --output-collapsed true|false.
        #[arg(long)]
        output_collapsed: Option<bool>,
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
    /// Run a cell in the hosting browser and wait for its result.
    ///
    /// Unexecuted prerequisite cells cascade first, exactly as they do for a
    /// click on Run. The result reports one of: executed, `execution_error`,
    /// `compile_error`, or `prerequisite_failed`.
    Run {
        /// Cell ID.
        cell_id: String,
        /// Return as soon as the run is queued, without waiting for a result.
        #[arg(long)]
        no_wait: bool,
        /// Seconds to wait for the execution result (covers a cold compile).
        #[arg(long, default_value_t = 360)]
        timeout_secs: u64,
    },
}

// ── Exit codes ──────────────────────────────────────────────────────────────

/// Named exit codes for consistent CLI error reporting.
#[repr(i32)]
enum CliExitCode {
    GenericError = 1,
    VersionConflict = 2,
    PermissionDenied = 3,
    ConnectionError = 4,
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
                        // The OS may have recycled the pidfile's PID for an
                        // unrelated process; verify it's actually our daemon
                        // before signaling so we don't SIGTERM a stranger.
                        if !is_ironpad_daemon(pid) {
                            eprintln!(
                                "pidfile PID {pid} is not the ironpad daemon (stale pidfile); not sending a signal"
                            );
                            let _ = tokio::fs::remove_file(&pid_path).await;
                            std::process::exit(1);
                        }
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
            let args: serde_json::Value = match serde_json::from_str(&args) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("invalid JSON args: {e}");
                    std::process::exit(1);
                }
            };
            let response = send_ipc(&command, args).await;
            print_response(&response);
        }
    }
}

// ── Cell command dispatch ───────────────────────────────────────────────────

async fn handle_cells_command(cmd: CellsCommand) {
    match cmd {
        CellsCommand::List => handle_cells_list().await,
        CellsCommand::Get { cell_id } => handle_cells_get(&cell_id).await,
        CellsCommand::Add {
            source,
            source_file,
            r#type,
            label,
            after,
            cargo_toml,
            shared,
        } => {
            handle_cells_add(
                source,
                source_file,
                r#type,
                label,
                after,
                cargo_toml,
                shared,
            )
            .await;
        }
        CellsCommand::Update {
            cell_id,
            source,
            source_file,
            cargo_toml,
            label,
            shared,
            collapsed,
            output_collapsed,
            version,
        } => {
            handle_cells_update(CellsUpdateArgs {
                cell_id,
                source,
                source_file,
                cargo_toml,
                label,
                shared,
                collapsed,
                output_collapsed,
                version,
            })
            .await;
        }
        CellsCommand::Delete { cell_id, version } => {
            handle_cells_delete(cell_id, version).await;
        }
        CellsCommand::Reorder { cell_ids } => handle_cells_reorder(cell_ids).await,
        CellsCommand::Run {
            cell_id,
            no_wait,
            timeout_secs,
        } => handle_cells_run(&cell_id, no_wait, timeout_secs).await,
    }
}

async fn handle_cells_run(cell_id: &str, no_wait: bool, timeout_secs: u64) {
    let response = send_ipc(
        "cells.run",
        serde_json::json!({
            "cell_id": cell_id,
            "wait": !no_wait,
            "timeout_secs": timeout_secs,
        }),
    )
    .await;
    print_response(&response);
}

async fn handle_cells_list() {
    let response = send_ipc("cells.list", serde_json::Value::Null).await;
    print_response(&response);
}

async fn handle_cells_get(cell_id: &str) {
    let response = send_ipc("cells.get", serde_json::json!({ "cell_id": cell_id })).await;
    print_response(&response);
}

#[allow(clippy::too_many_arguments)] // Mirrors the clap arg list one-to-one.
async fn handle_cells_add(
    source: Option<String>,
    source_file: Option<String>,
    r#type: CellTypeArg,
    label: Option<String>,
    after: Option<String>,
    cargo_toml: Option<String>,
    shared: bool,
) {
    let source = resolve_source(source, source_file);
    let response = send_ipc(
        "cells.add",
        serde_json::json!({
            "source": source.unwrap_or_default(),
            "type": r#type.as_str(),
            "label": label
                .unwrap_or_else(|| ironpad_common::protocol::DEFAULT_CELL_LABEL.to_string()),
            "after_cell_id": after,
            "cargo_toml": cargo_toml,
            "shared": shared,
        }),
    )
    .await;
    print_response(&response);
}

/// Arg bundle for [`handle_cells_update`] — mirrors the clap arg list.
struct CellsUpdateArgs {
    cell_id: String,
    source: Option<String>,
    source_file: Option<String>,
    cargo_toml: Option<String>,
    label: Option<String>,
    shared: Option<bool>,
    collapsed: Option<bool>,
    output_collapsed: Option<bool>,
    version: Option<u64>,
}

impl CellsUpdateArgs {
    /// True when no field flag was supplied — the update would be a no-op
    /// that still bumps the cell version.
    fn is_empty_update(&self) -> bool {
        self.source.is_none()
            && self.source_file.is_none()
            && self.cargo_toml.is_none()
            && self.label.is_none()
            && self.shared.is_none()
            && self.collapsed.is_none()
            && self.output_collapsed.is_none()
    }
}

async fn handle_cells_update(update: CellsUpdateArgs) {
    // Reject a field-less update up front: it would round-trip an all-None
    // mutation that reports success while changing nothing user-visible
    // (except bumping the version).
    if update.is_empty_update() {
        eprintln!(
            "error: nothing to update — pass at least one of --source, --source-file, \
             --cargo-toml, --label, --shared, --collapsed, --output-collapsed"
        );
        std::process::exit(CliExitCode::GenericError as i32);
    }

    let source = resolve_source(update.source, update.source_file);

    let version = match update.version {
        Some(v) => v,
        None => fetch_cell_version(&update.cell_id).await,
    };

    let mut args = serde_json::json!({
        "cell_id": update.cell_id,
        "version": version,
    });
    if let Some(src) = source {
        args["source"] = serde_json::Value::String(src);
    }
    if let Some(ct) = update.cargo_toml {
        args["cargo_toml"] = serde_json::Value::String(ct);
    }
    if let Some(lbl) = update.label {
        args["label"] = serde_json::Value::String(lbl);
    }
    if let Some(sh) = update.shared {
        args["shared"] = serde_json::Value::Bool(sh);
    }
    if let Some(c) = update.collapsed {
        args["collapsed"] = serde_json::Value::Bool(c);
    }
    if let Some(oc) = update.output_collapsed {
        args["output_collapsed"] = serde_json::Value::Bool(oc);
    }

    let response = send_ipc("cells.update", args).await;
    print_response(&response);
}

async fn handle_cells_delete(cell_id: String, version: Option<u64>) {
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

async fn handle_cells_reorder(cell_ids: Vec<String>) {
    let response = send_ipc("cells.reorder", serde_json::json!({ "cell_ids": cell_ids })).await;
    print_response(&response);
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
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0)
}

/// Resolve source from --source, --source-file, or stdin ("-").
fn resolve_source(source: Option<String>, source_file: Option<String>) -> Option<String> {
    if let Some(ref s) = source {
        if s == "-" {
            let mut buf = String::new();
            if let Err(e) = std::io::stdin().read_to_string(&mut buf) {
                eprintln!("failed to read stdin: {e}");
                std::process::exit(1);
            }
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
            Some("VersionConflict") => CliExitCode::VersionConflict,
            Some("PermissionDenied") => CliExitCode::PermissionDenied,
            Some(c) if c.contains("connect") || c.contains("disconnect") => {
                CliExitCode::ConnectionError
            }
            _ => {
                if response
                    .error
                    .as_deref()
                    .is_some_and(|e| e.contains("daemon") || e.contains("socket"))
                {
                    CliExitCode::ConnectionError
                } else {
                    CliExitCode::GenericError
                }
            }
        };
        std::process::exit(exit_code as i32);
    }
}

// ── IPC client ──────────────────────────────────────────────────────────────

/// Send a command to the daemon via Unix socket and return the response.
async fn send_ipc(command: &str, args: serde_json::Value) -> IpcResponse {
    let sock = daemon::socket_path();

    let Ok(stream) = UnixStream::connect(&sock).await else {
        return IpcResponse::error_with_code(
            "daemon is not running (cannot connect to socket)",
            "connection_error",
        );
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

    let mut reader = BufReader::new(reader);
    match crate::ipc::read_frame(&mut reader).await {
        Ok(Some(line)) => serde_json::from_str(&line)
            .unwrap_or_else(|_| IpcResponse::error("invalid response from daemon")),
        _ => IpcResponse::error("no response from daemon"),
    }
}

// ── Signal helper ───────────────────────────────────────────────────────────

/// Whether `pid` looks like an ironpad daemon, checked via `/proc/<pid>/cmdline`
/// so a recycled PID (some unrelated process) isn't signaled. On systems
/// without `/proc` (non-Linux) we can't verify, so fall back to allowing the
/// signal — matching the previous behavior there.
fn is_ironpad_daemon(pid: u32) -> bool {
    match std::fs::read(format!("/proc/{pid}/cmdline")) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).contains("ironpad"),
        // `/proc` absent → non-Linux, can't verify → allow (old behavior).
        // `/proc` present but this PID gone → the process is dead anyway.
        Err(_) => !std::path::Path::new("/proc").exists(),
    }
}

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

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_update_parses_collapse_and_shared_flags() {
        let cli = Cli::try_parse_from([
            "ironpad",
            "cells",
            "update",
            "c1",
            "--shared",
            "true",
            "--collapsed",
            "true",
            "--output-collapsed",
            "false",
        ])
        .expect("flags should parse");
        let Command::Cells(CellsCommand::Update {
            shared,
            collapsed,
            output_collapsed,
            ..
        }) = cli.command
        else {
            panic!("expected cells update");
        };
        assert_eq!(shared, Some(true));
        assert_eq!(collapsed, Some(true));
        assert_eq!(output_collapsed, Some(false));
    }

    #[test]
    fn cells_add_parses_shared_flag() {
        let cli = Cli::try_parse_from(["ironpad", "cells", "add", "--source", "42", "--shared"])
            .expect("flag should parse");
        let Command::Cells(CellsCommand::Add { shared, .. }) = cli.command else {
            panic!("expected cells add");
        };
        assert!(shared);
    }

    #[test]
    fn empty_update_is_detected() {
        // No field flags → rejected before any IPC round-trip.
        let empty = CellsUpdateArgs {
            cell_id: "c1".into(),
            source: None,
            source_file: None,
            cargo_toml: None,
            label: None,
            shared: None,
            collapsed: None,
            output_collapsed: None,
            version: None,
        };
        assert!(empty.is_empty_update());

        let with_flag = CellsUpdateArgs {
            collapsed: Some(true),
            ..empty
        };
        assert!(!with_flag.is_empty_update());
    }
}
