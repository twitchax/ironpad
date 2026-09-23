/**
 * `BridgeExecutor` fallback tests (executor-bridge.js).
 *
 *   cargo make test-js
 *
 * loadBlob, execute, tick and tickLive share ONE policy for a failed worker
 * call: retry on the main-thread executor, tagging the result `fallback:
 * true`, except for a deliberate cancellation. These run the real bridge
 * against a fake Worker, with the worker made to fail by parking it (the
 * state the respawn cap leaves behind) or by answering with an error.
 */

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";

import { loadCore, makeMemory, rawEntry, writeTickResult } from "./executor-harness.mjs";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const BRIDGE_SOURCE = readFileSync(
  path.join(HERE, "..", "..", "public", "executor-bridge.js"),
  "utf8",
);

// ── Harness ─────────────────────────────────────────────────────────────────

/** A fresh bridge over a fake Worker that records what it is sent. */
function loadBridge() {
  const workers = [];
  class FakeWorker {
    constructor(url) {
      this.url = url;
      this.posted = [];
      workers.push(this);
    }
    postMessage(msg) {
      this.posted.push(msg);
    }
    terminate() {
      this.terminated = true;
    }
  }
  const warnings = [];
  const fakeConsole = {
    log() {},
    error() {},
    warn: (...args) => warnings.push(args.join(" ")),
  };
  const window = {};
  const document = { currentScript: null, querySelector: () => null };
  // eslint-disable-next-line no-new-func
  new Function("window", "document", "Worker", "console", BRIDGE_SOURCE)(
    window,
    document,
    FakeWorker,
    fakeConsole,
  );
  return { bridge: window.IronpadExecutor, workers, warnings };
}

/** A main-thread executor stand-in that records every call it receives. */
function fakeMainExecutor({ without = [] } = {}) {
  const exec = {
    calls: [],
    loaded: new Map(),
    isLoaded(cellId, hash) {
      return exec.loaded.get(cellId) === hash;
    },
    async loadBlob(cellId, hash) {
      exec.calls.push(["loadBlob", cellId, hash]);
      exec.loaded.set(cellId, hash);
    },
    async execute(cellId, inputBytes) {
      exec.calls.push(["execute", cellId, Array.from(inputBytes)]);
      return { outputBytes: new Uint8Array(0), displayText: null, typeTag: null };
    },
    async tick(cellId) {
      exec.calls.push(["tick", cellId]);
      return { width: 1, height: 1, rgbBytes: new Uint8Array(3) };
    },
    async tickLive(cellId) {
      exec.calls.push(["tickLive", cellId]);
      return { kind: 0, content: "x" };
    },
    onHostMessage() {},
    unload() {},
  };
  for (const name of without) delete exec[name];
  return exec;
}

/** Park the worker the way the respawn cap does: every request now rejects. */
function park(bridge) {
  bridge._worker = null;
}

// ── Tests ───────────────────────────────────────────────────────────────────

test("every call path falls back to the main thread and tags its result", async () => {
  const { bridge, warnings } = loadBridge();
  const main = fakeMainExecutor();
  bridge._mainExecutor = main;
  park(bridge);

  await bridge.loadBlob("c1", "h1", new Uint8Array([0]), null);
  const executed = await bridge.execute("c1", new Uint8Array([7]));
  const frame = await bridge.tick("c1");
  const view = await bridge.tickLive("c1");

  assert.equal(executed.fallback, true);
  assert.equal(frame.fallback, true);
  assert.equal(view.fallback, true);
  assert.deepEqual(main.calls, [
    ["loadBlob", "c1", "h1"],
    ["execute", "c1", [7]],
    ["tick", "c1"],
    ["tickLive", "c1"],
  ], "the blob loads once, and each call reaches the main thread with its args");
  assert.equal(warnings.length, 4, "one warning per fallback");
  assert.match(warnings[2], /worker tick failed for c1, retrying on main thread/);
});

test("a loadBlob fallback leaves the worker's loaded cache alone", async () => {
  // _loadedCache tracks what the WORKER holds; claiming the blob there would
  // make a respawned worker skip a load it never did.
  const { bridge } = loadBridge();
  bridge._mainExecutor = fakeMainExecutor();
  park(bridge);

  const loaded = await bridge.loadBlob("c1", "h1", new Uint8Array([0]), null);

  assert.equal(loaded, undefined);
  assert.equal(bridge.isLoaded("c1", "h1"), false);
});

test("a worker error reply falls back too, not only a parked worker", async () => {
  const { bridge, workers } = loadBridge();
  const main = fakeMainExecutor();
  bridge._mainExecutor = main;
  bridge._blobCache.set("c1", { hash: "h1", wasmBytes: new Uint8Array(0), jsGlue: null });

  const pending = bridge.tick("c1");
  const request = workers[0].posted.at(-1);
  assert.equal(request.type, "tick");
  bridge._onWorkerMessage({ type: "error", id: request.id, error: "unreachable" });

  assert.equal((await pending).fallback, true);
  assert.deepEqual(main.calls.map((c) => c[0]), ["loadBlob", "tick"]);
});

test("a terminate() cancellation is rethrown, never re-run on the main thread", async () => {
  // Re-running a runaway cell on the main thread would freeze the tab.
  const { bridge, warnings } = loadBridge();
  const main = fakeMainExecutor();
  bridge._mainExecutor = main;
  bridge._blobCache.set("c1", { hash: "h1", wasmBytes: new Uint8Array(0), jsGlue: null });

  const pending = bridge.tickLive("c1");
  bridge.terminate();

  await assert.rejects(pending, (e) => e.name === "AbortError");
  assert.deepEqual(main.calls, []);
  assert.deepEqual(warnings, []);
});

test("with no cached blob, or no such method on the main thread, the worker error surfaces", async () => {
  const { bridge } = loadBridge();
  bridge._mainExecutor = fakeMainExecutor({ without: ["tickLive"] });
  park(bridge);

  await assert.rejects(bridge.tick("never-loaded"), /Worker unavailable/);

  await bridge.loadBlob("c1", "h1", new Uint8Array([0]), null);
  await assert.rejects(bridge.tickLive("c1"), /Worker unavailable/);
});

test("the fallback drives the real core executor's tick", async () => {
  // End to end on the core side: the bridge hands the call to a CellExecutor,
  // whose raw tick path reads the frame out of the cell's memory.
  const { bridge } = loadBridge();
  const { executor } = loadCore();
  const cell = makeMemory();
  const retptr = writeTickResult(cell);
  executor.modules.set("sim", rawEntry(cell, { cell_tick: () => retptr }));
  executor.modules.get("sim").hash = "h-sim";
  bridge._mainExecutor = executor;
  bridge._blobCache.set("sim", { hash: "h-sim", wasmBytes: new Uint8Array(0), jsGlue: null });
  park(bridge);

  const frame = await bridge.tick("sim");

  assert.equal(frame.fallback, true);
  assert.deepEqual(Array.from(frame.rgbBytes), [10, 20, 30]);
});
