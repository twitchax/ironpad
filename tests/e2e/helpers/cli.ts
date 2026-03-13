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
  });

  // Wait for the socket to appear (daemon is ready).
  await waitForSocket(sockPath, 15_000);

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

/** Wait for a Unix socket file to appear. */
async function waitForSocket(
  sockPath: string,
  timeoutMs: number
): Promise<void> {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    if (fs.existsSync(sockPath)) {
      return;
    }
    await new Promise((r) => setTimeout(r, 200));
  }
  throw new Error(`Timed out waiting for daemon socket at ${sockPath}`);
}
