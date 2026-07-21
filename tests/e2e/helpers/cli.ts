/**
 * Helpers for spawning and interacting with the ironpad-cli daemon in tests.
 */
import { spawn, ChildProcess, execFileSync } from "child_process";
import * as path from "path";
import * as fs from "fs";
import * as os from "os";

const CLI_BIN = path.join(process.cwd(), "target", "release", "ironpad-cli");

// Hermetic isolation: the daemon resolves its runtime dir from `$HOME/.ironpad`
// (see ironpad-cli `daemon_dir()`). Point every CLI invocation at a private
// temp HOME so tests never touch the developer's real `~/.ironpad` — or collide
// with a running daemon / stale socket there. One dir per test process, shared
// by the serial session suite.
const TEST_HOME = fs.mkdtempSync(path.join(os.tmpdir(), "ironpad-e2e-home-"));
const DAEMON_DIR = path.join(TEST_HOME, ".ironpad");
const CLI_ENV = { ...process.env, HOME: TEST_HOME };

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
    env: { ...CLI_ENV, RUST_LOG: "ironpad=debug" },
  });

  // Surface daemon errors in test output for debugging.
  child.stderr?.on("data", (data: Buffer) => {
    console.error(`[daemon] ${data.toString().trimEnd()}`);
  });

  // Wait for the daemon to be fully connected AND have the notebook cached —
  // connected alone races the async cache fill (see waitForNotebookCached).
  await waitForDaemonReady(sockPath, 30_000);
  await waitForNotebookCached(10_000);

  return { process: child, token };
}

/** Execute a CLI command and return parsed JSON response. */
export function cliExec(command: string[]): any {
  // execFileSync (no shell) passes each element as a literal argv entry, so
  // callers never need to shell-quote args that contain spaces.
  const result = execFileSync(CLI_BIN, command, {
    encoding: "utf-8",
    timeout: 15_000,
    env: CLI_ENV,
  });
  return JSON.parse(result.trim());
}

/** Execute a CLI command, returning { stdout, stderr, exitCode }. */
export function cliExecRaw(
  command: string[]
): { stdout: string; stderr: string; exitCode: number } {
  try {
    const stdout = execFileSync(CLI_BIN, command, {
      encoding: "utf-8",
      timeout: 15_000,
      env: CLI_ENV,
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
    execFileSync(CLI_BIN, ["daemon-stop"], {
      encoding: "utf-8",
      timeout: 5_000,
      env: CLI_ENV,
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
        execFileSync(CLI_BIN, ["status"], {
          encoding: "utf-8",
          timeout: 5_000,
          env: CLI_ENV,
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

/**
 * Phase 3: `connected` only means the WebSocket handshake finished — the
 * daemon populates its notebook cache from an async NotebookGet AFTER that,
 * and the Unix socket (deliberately) serves commands the whole time. Poll
 * until `notebook` succeeds so tests can't race the cache fill ("no
 * notebook cached").
 */
async function waitForNotebookCached(timeoutMs: number): Promise<void> {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    try {
      // Success prints the notebook JSON and exits 0; "no notebook cached"
      // exits non-zero (execFileSync throws). Exit code IS the signal.
      execFileSync(CLI_BIN, ["notebook"], {
        encoding: "utf-8",
        timeout: 5_000,
        env: CLI_ENV,
      });
      return;
    } catch {
      // Not cached yet — retry.
    }
    await new Promise((r) => setTimeout(r, 200));
  }
  throw new Error("Timed out waiting for daemon to cache the notebook");
}
