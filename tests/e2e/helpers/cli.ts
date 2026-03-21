/**
 * Helpers for spawning and interacting with the ironpad-cli daemon in tests.
 */
import { spawn, ChildProcess, execSync } from "child_process";
import * as path from "path";
import * as fs from "fs";
import * as os from "os";

const CLI_BIN = path.join(process.cwd(), "target", "release", "ironpad-cli");
const DAEMON_DIR = path.join(os.homedir(), ".ironpad");

export interface CliHandle {
  process: ChildProcess;
  token: string;
}

/** Start the CLI daemon and wait for it to be ready. */
export async function connectCli(
  token: string,
  host: string = "ws://localhost:3111"
): Promise<CliHandle> {
  // Clean up stale socket/pid.
  const sockPath = path.join(DAEMON_DIR, "daemon.sock");
  const pidPath = path.join(DAEMON_DIR, "daemon.pid");
  try {
    fs.unlinkSync(sockPath);
  } catch {}
  try {
    fs.unlinkSync(pidPath);
  } catch {}

  const child = spawn(CLI_BIN, ["--host", host, "--token", token, "daemon"], {
    stdio: ["ignore", "pipe", "pipe"],
    detached: true,
    env: { ...process.env, RUST_LOG: "ironpad=debug" },
  });

  // Surface daemon errors in test output for debugging.
  child.stderr?.on("data", (data: Buffer) => {
    console.error(`[daemon] ${data.toString().trimEnd()}`);
  });

  // Wait for the daemon to be fully connected.
  await waitForDaemonReady(sockPath, 30_000);

  return { process: child, token };
}

/** Execute a CLI command and return parsed JSON response. */
export function cliExec(command: string[]): any {
  const result = execSync([CLI_BIN, ...command].join(" "), {
    encoding: "utf-8",
    timeout: 15_000,
  });
  return JSON.parse(result.trim());
}

/** Execute a CLI command, returning { stdout, stderr, exitCode }. */
export function cliExecRaw(
  command: string[]
): { stdout: string; stderr: string; exitCode: number } {
  try {
    const stdout = execSync([CLI_BIN, ...command].join(" "), {
      encoding: "utf-8",
      timeout: 15_000,
    });
    return { stdout: stdout.trim(), stderr: "", exitCode: 0 };
  } catch (e: any) {
    return {
      stdout: (e.stdout || "").trim(),
      stderr: (e.stderr || "").trim(),
      exitCode: e.status || 1,
    };
  }
}

/** Stop the daemon gracefully. */
export function stopCli(handle: CliHandle): void {
  try {
    execSync([CLI_BIN, "daemon-stop"].join(" "), {
      encoding: "utf-8",
      timeout: 5_000,
    });
  } catch {}
  try {
    handle.process.kill("SIGTERM");
  } catch {}
}

/** Wait for the daemon socket to appear and report connected status. */
async function waitForDaemonReady(
  sockPath: string,
  timeoutMs: number
): Promise<void> {
  const start = Date.now();

  // Phase 1: wait for the socket file to exist.
  while (Date.now() - start < timeoutMs) {
    if (fs.existsSync(sockPath)) {
      break;
    }
    await new Promise((r) => setTimeout(r, 200));
  }
  if (!fs.existsSync(sockPath)) {
    throw new Error(`Timed out waiting for daemon socket at ${sockPath}`);
  }

  // Phase 2: poll `status` until the daemon reports connected.
  while (Date.now() - start < timeoutMs) {
    try {
      const status = JSON.parse(
        execSync([CLI_BIN, "status"].join(" "), {
          encoding: "utf-8",
          timeout: 5_000,
        }).trim()
      );
      if (status.connected) {
        return;
      }
    } catch {
      // Daemon not ready yet — retry.
    }
    await new Promise((r) => setTimeout(r, 200));
  }
  throw new Error("Timed out waiting for daemon to report connected status");
}
