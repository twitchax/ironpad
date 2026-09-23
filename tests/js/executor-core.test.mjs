/**
 * `CellExecutor` unit tests (executor-core.js + the worker entry).
 *
 *   cargo make test-js
 *
 * These load the real executor scripts from `public/` and drive the methods a
 * cell's WASM imports and the bridge call, against FAKE loaded entries: a
 * `{ type: "raw", instance: { exports } }` or `{ type: "bindgen", module,
 * wasm }` record in `executor.modules`, whose memory is a real
 * `WebAssembly.Memory` and whose `ironpad_alloc`/`cell_tick` are plain JS. No
 * cell is compiled and no browser runs, which is the point: the ABI details
 * under test (length prefixes, result-struct layouts, which pointer is freed
 * with which size) are invisible to a Playwright spec until they corrupt a
 * frame.
 *
 * The scripts run with `TextDecoder` bound to `BrowserTextDecoder`, which
 * refuses views over a SharedArrayBuffer the way browsers do. Node's own
 * decoder accepts them, so without it a live-view decode of a rayon cell's
 * shared memory (a trap inside a WASM import, in a browser) would pass here.
 */

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const PUBLIC = path.join(HERE, "..", "..", "public");

// Mirrors of the executor's result-struct sizes (executor-core.js). Duplicated
// deliberately: the assertions are about the ABI, so reading the sizes out of
// the module under test would make a wrong size agree with itself.
const TICK_RESULT_SIZE = 16;
const LIVE_TICK_RESULT_SIZE = 12;

// ── Harness ─────────────────────────────────────────────────────────────────

function isShared(buffer) {
  // Realm-agnostic: the memory may come from any realm's WebAssembly.
  return Object.prototype.toString.call(buffer) === "[object SharedArrayBuffer]";
}

/** A `TextDecoder` with the browser's shared-memory rule and a call counter. */
function makeBrowserTextDecoder(stats) {
  return class BrowserTextDecoder {
    constructor() {
      this.inner = new TextDecoder();
    }

    decode(input) {
      stats.decodes += 1;
      const buffer = input && ArrayBuffer.isView(input) ? input.buffer : input;
      if (isShared(buffer)) {
        throw new TypeError(
          "Failed to execute 'decode' on 'TextDecoder': The provided ArrayBufferView value must not be shared.",
        );
      }
      return this.inner.decode(input);
    }
  };
}

function source(name) {
  return readFileSync(path.join(PUBLIC, name), "utf8");
}

/**
 * Evaluate public scripts against one fake global. Each script sees `self`,
 * `importScripts`, `TextDecoder` and `console` as that global's, which is all
 * the executor chain reaches for at load time.
 */
function makeGlobal() {
  const stats = { decodes: 0, warnings: [] };
  const posted = [];
  const g = {
    location: { search: "" },
    postMessage: (msg) => posted.push(msg),
  };
  g.self = g;
  g.constructor = function WorkerGlobalScope() {};
  const fakeConsole = {
    log() {},
    error() {},
    warn: (...args) => stats.warnings.push(args.join(" ")),
  };
  const Decoder = makeBrowserTextDecoder(stats);

  function run(name) {
    // eslint-disable-next-line no-new-func
    new Function("self", "importScripts", "TextDecoder", "console", source(name))(
      g,
      importScripts,
      Decoder,
      fakeConsole,
    );
  }
  function importScripts(url) {
    run(url.replace(/^\//, "").replace(/\?.*$/, ""));
  }

  return { g, run, stats, posted };
}

/** The core executor, loaded exactly as the main-thread fallback chain does. */
function loadCore() {
  const env = makeGlobal();
  env.run("executor-gpu.js");
  env.run("executor-glue.js");
  env.run("executor-core.js");
  const executor = new env.g.__IronpadExecutorCore.CellExecutor("self._ironpadExecutor");
  return { ...env, executor };
}

/** The worker entry, loaded through its own importScripts chain. */
function loadWorker() {
  const env = makeGlobal();
  env.run("executor-worker.js");
  const executor = env.g._ironpadExecutor;
  assert.ok(executor, "executor-worker.js must create the executor on self");
  return { ...env, executor };
}

/**
 * Linear memory plus the two allocator exports a cell provides. `alloc` is a
 * bump allocator that logs its requests; `failAlloc` makes it return 0.
 */
function makeMemory({ shared = false } = {}) {
  const memory = shared
    ? new WebAssembly.Memory({ initial: 1, maximum: 1, shared: true })
    : new WebAssembly.Memory({ initial: 1 });
  const log = { allocs: [], deallocs: [] };
  let next = 4096;
  const cell = {
    memory,
    log,
    failAlloc: false,
    ironpad_alloc(n) {
      log.allocs.push(n);
      if (cell.failAlloc) return 0;
      const ptr = next;
      next += (n + 7) & ~7;
      return ptr;
    },
    ironpad_dealloc(ptr, len) {
      log.deallocs.push([ptr, len]);
    },
    /** Write bytes at a fixed address and return that address. */
    put(ptr, bytes) {
      new Uint8Array(memory.buffer, ptr, bytes.length).set(bytes);
      return ptr;
    },
    putText(ptr, text) {
      const bytes = new TextEncoder().encode(text);
      cell.put(ptr, bytes);
      return { ptr, len: bytes.length };
    },
    putU32s(ptr, words) {
      const view = new DataView(memory.buffer);
      words.forEach((w, i) => view.setUint32(ptr + i * 4, w, true));
      return ptr;
    },
    readLengthPrefixed(ptr) {
      const len = new DataView(memory.buffer).getUint32(ptr, true);
      const bytes = new Uint8Array(memory.buffer, ptr + 4, len).slice();
      return new TextDecoder().decode(bytes);
    },
  };
  return cell;
}

function rawEntry(cell, extraExports = {}) {
  return {
    hash: "h",
    type: "raw",
    needsJspi: false,
    instance: {
      exports: {
        memory: cell.memory,
        ironpad_alloc: cell.ironpad_alloc,
        ironpad_dealloc: cell.ironpad_dealloc,
        ...extraExports,
      },
    },
  };
}

function bindgenEntry(cell, moduleExports = {}, extraWasm = {}) {
  return {
    hash: "h",
    type: "bindgen",
    needsJspi: false,
    module: moduleExports,
    wasm: {
      memory: cell.memory,
      ironpad_alloc: cell.ironpad_alloc,
      ironpad_dealloc: cell.ironpad_dealloc,
      ...extraWasm,
    },
  };
}

// ── Sim bus reads (review js-2) ─────────────────────────────────────────────

test("_simRead writes the latest value as [u32-LE length][JSON bytes]", () => {
  const { executor } = loadCore();
  const cell = makeMemory({ shared: true });
  executor.modules.set("c1", rawEntry(cell));
  executor.simBusWrite("speed", { v: 1 });
  executor.simBusWrite("speed", { v: "é2" });

  const key = cell.putText(64, "speed");
  const ptr = executor._simRead("c1", key.ptr, key.len);

  assert.notEqual(ptr, 0);
  const payload = new TextEncoder().encode('{"v":"é2"}');
  assert.deepEqual(cell.log.allocs, [4 + payload.length], "one alloc: prefix plus payload");
  assert.equal(new DataView(cell.memory.buffer).getUint32(ptr, true), payload.length);
  assert.deepEqual(new Uint8Array(cell.memory.buffer, ptr + 4, payload.length).slice(), payload);
});

test("_simReadAll writes the whole ring as a JSON array", () => {
  const { executor } = loadCore();
  const cell = makeMemory();
  executor.modules.set("c1", bindgenEntry(cell));
  executor.simBusWrite("xs", 1);
  executor.simBusWrite("xs", 2);
  executor.simBusWrite("xs", 3);

  const key = cell.putText(64, "xs");
  const ptr = executor._simReadAll("c1", key.ptr, key.len);

  assert.equal(cell.readLengthPrefixed(ptr), "[1,2,3]");
});

test("sim reads return 0 for an unknown key, a failed alloc, or a missing cell", () => {
  const { executor } = loadCore();
  const cell = makeMemory();
  executor.modules.set("c1", rawEntry(cell));
  executor.simBusWrite("speed", 1);

  const unknown = cell.putText(64, "nope");
  assert.equal(executor._simRead("c1", unknown.ptr, unknown.len), 0);
  assert.equal(executor._simReadAll("c1", unknown.ptr, unknown.len), 0);
  assert.deepEqual(cell.log.allocs, [], "nothing to write, so nothing allocated");

  cell.failAlloc = true;
  const key = cell.putText(64, "speed");
  assert.equal(executor._simRead("c1", key.ptr, key.len), 0);
  assert.equal(executor._simReadAll("c1", key.ptr, key.len), 0);

  assert.equal(executor._simRead("missing", key.ptr, key.len), 0);
  assert.equal(executor._simReadAll("missing", key.ptr, key.len), 0);
});

test("_cellMemory resolves either loading path and tolerates a half-built entry", () => {
  const { executor } = loadCore();
  const cell = makeMemory();
  executor.modules.set("raw", rawEntry(cell));
  executor.modules.set("bindgen", bindgenEntry(cell));
  executor.modules.set("broken", { hash: "h", type: "raw" });

  assert.equal(executor._cellMemory("raw"), cell.memory);
  assert.equal(executor._cellMemory("bindgen"), cell.memory);
  assert.equal(executor._cellMemory("broken"), null);
  assert.equal(executor._cellMemory("missing"), null);
  assert.equal(executor._blockingRead("broken", 0, 8), 0, "a blocking read on it copies nothing");
});
