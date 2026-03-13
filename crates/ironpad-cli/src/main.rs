mod daemon;
mod ipc;

use clap::{Parser, Subcommand};
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
    /// Show daemon status.
    Status,
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

// ── Main ────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    match cli.command {
        Command::Daemon => {
            // Initialize tracing for daemon mode (logs to stderr).
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
            // Read pidfile and send SIGTERM.
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
            println!("{}", serde_json::to_string_pretty(&response).unwrap());
        }

        Command::Raw { command, args } => {
            let args: serde_json::Value = serde_json::from_str(&args).unwrap_or_default();
            let response = send_ipc(&command, args).await;
            println!("{}", serde_json::to_string(&response).unwrap());
        }
    }
}

// ── IPC client ──────────────────────────────────────────────────────────────

/// Send a command to the daemon via Unix socket and return the response.
async fn send_ipc(command: &str, args: serde_json::Value) -> IpcResponse {
    let sock = daemon::socket_path();

    let stream = match UnixStream::connect(&sock).await {
        Ok(s) => s,
        Err(_) => {
            return IpcResponse::error("daemon is not running (cannot connect to socket)");
        }
    };

    let (reader, mut writer) = stream.into_split();

    let req = IpcRequest {
        command: command.to_string(),
        args,
    };

    let mut json = serde_json::to_string(&req).unwrap();
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
