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

// ── Host messages (review js-1) ─────────────────────────────────────────────

test("a host message in shared memory reaches its handler once, parsed", () => {
  // The rayon case: a cell's memory is a SharedArrayBuffer, and a browser
  // TextDecoder throws on a live view of one. The read must go through a copy.
  const { executor, stats } = loadCore();
  const cell = makeMemory({ shared: true });
  executor.modules.set("c1", rawEntry(cell));

  const calls = [];
  executor.onHostMessage("progress_update", (msg, cellId) => calls.push({ msg, cellId }));

  const { ptr, len } = cell.putText(64, '{"type":"progress_update","id":"p1","value":50}');
  executor._dispatchHostMessage("c1", ptr, len);

  assert.deepEqual(calls, [
    { msg: { type: "progress_update", id: "p1", value: 50 }, cellId: "c1" },
  ]);
  assert.equal(stats.decodes, 1);
});

test("a host message for an unloaded cell is dropped without reading memory", () => {
  const { executor, stats } = loadCore();
  let called = false;
  executor.onHostMessage("progress_update", () => {
    called = true;
  });

  executor._dispatchHostMessage("missing", 0, 10);

  assert.equal(called, false);
  assert.equal(stats.decodes, 0);
});

test("a malformed host message warns instead of trapping the cell", () => {
  // The handler runs inside a WASM import: a throw there traps the cell.
  const { executor, stats } = loadCore();
  const cell = makeMemory();
  executor.modules.set("c1", rawEntry(cell));

  const { ptr, len } = cell.putText(64, "{not json");
  assert.doesNotThrow(() => executor._dispatchHostMessage("c1", ptr, len));
  assert.equal(stats.warnings.length, 1);
  assert.match(stats.warnings[0], /failed to parse host message/);
});

test("the worker forwards each host message once, decoded once, before local dispatch", () => {
  // The worker used to re-resolve memory and decode a live view itself, then
  // call the core, which decoded a second time. The forward now rides the
  // core's single copy-and-decode, so shared memory is safe here too.
  const { executor, stats, posted } = loadWorker();
  const cell = makeMemory({ shared: true });
  executor.modules.set("c1", bindgenEntry(cell));

  const order = [];
  executor.onHostMessage("progress_update", (msg) => order.push(["handler", msg.value, posted.length]));

  const text = '{"type":"progress_update","id":"p1","value":75}';
  const { ptr, len } = cell.putText(128, text);
  executor._dispatchHostMessage("c1", ptr, len);

  assert.deepEqual(posted, [{ type: "hostMessage", cellId: "c1", messageJson: text }]);
  assert.deepEqual(order, [["handler", 75, 1]], "forwarded to the main thread first");
  assert.equal(stats.decodes, 1, "read and decoded exactly once");
});

test("the worker's own sim_emit handler updates its bus from a forwarded message", () => {
  const { executor, posted } = loadWorker();
  const cell = makeMemory({ shared: true });
  executor.modules.set("c1", rawEntry(cell));

  const text = '{"type":"sim_emit","key":"speed","value":3.5}';
  const { ptr, len } = cell.putText(64, text);
  executor._dispatchHostMessage("c1", ptr, len);

  assert.equal(posted.length, 1);
  assert.equal(executor._simBus.get("speed").latest, "3.5");
});

test("the worker forwards a gpu_read_pixels request and still defers it locally", () => {
  const { executor, stats, posted } = loadWorker();
  const cell = makeMemory();
  executor.modules.set("c1", rawEntry(cell));

  const text = '{"type":"gpu_read_pixels","output_handle":1,"staging_handle":2,"width":4,"height":4}';
  const { ptr, len } = cell.putText(64, text);
  executor._dispatchHostMessage("c1", ptr, len);

  assert.equal(posted.length, 1);
  assert.equal(executor._pendingGpuReadbacks.length, 1);
  assert.equal(executor._pendingGpuReadbacks[0].width, 4);
  assert.equal(stats.decodes, 1, "unshared memory is read once too");
});

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
