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
 * Both codecs also count their constructions, which the executor makes once
 * per script load rather than once per per-frame call.
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

/** A `TextDecoder` with the browser's shared-memory rule and counters. */
function makeBrowserTextDecoder(stats) {
  return class BrowserTextDecoder {
    constructor() {
      stats.codecs += 1;
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

/** A `TextEncoder` that counts its constructions. */
function makeCountingTextEncoder(stats) {
  return class CountingTextEncoder extends TextEncoder {
    constructor() {
      super();
      stats.codecs += 1;
    }
  };
}

function source(name) {
  return readFileSync(path.join(PUBLIC, name), "utf8");
}

/**
 * Evaluate public scripts against one fake global. Each script sees `self`,
 * `importScripts`, the two UTF-8 codecs and `console` as that global's, which
 * is all the executor chain reaches for at load time.
 */
function makeGlobal() {
  const stats = { decodes: 0, codecs: 0, warnings: [] };
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
  const Encoder = makeCountingTextEncoder(stats);

  function run(name) {
    // eslint-disable-next-line no-new-func
    new Function("self", "importScripts", "TextDecoder", "TextEncoder", "console", source(name))(
      g,
      importScripts,
      Decoder,
      Encoder,
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

/** A TickResult at 256 pointing at three RGB bytes at 512. */
function writeTickResult(cell) {
  cell.put(512, [10, 20, 30]);
  return cell.putU32s(256, [512, 3, 1, 1]);
}

/** A LiveTickResult at 256 (kind 1 = Html) pointing at UTF-8 content at 512. */
function writeLiveTickResult(cell, content) {
  const { len } = cell.putText(512, content);
  cell.putU32s(256, [1, 512, len]);
  return { ptr: 256, len };
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

// ── Ticks (review js-4) ─────────────────────────────────────────────────────

test("tick reads a raw cell's TickResult and frees it with the TickResult size", async () => {
  const { executor } = loadCore();
  const cell = makeMemory({ shared: true });
  const retptr = writeTickResult(cell);
  executor.modules.set("sim", rawEntry(cell, { cell_tick: () => retptr }));

  const frame = await executor.tick("sim");

  assert.equal(frame.width, 1);
  assert.equal(frame.height, 1);
  assert.deepEqual(Array.from(frame.rgbBytes), [10, 20, 30]);
  assert.deepEqual(cell.log.deallocs, [
    [512, 3],
    [retptr, TICK_RESULT_SIZE],
  ]);
  assert.deepEqual(cell.log.allocs, [], "a direct-return tick allocates nothing");
});

test("tickLive reads a raw cell's LiveTickResult and frees it with its own size", async () => {
  const { executor } = loadCore();
  const cell = makeMemory({ shared: true });
  const result = writeLiveTickResult(cell, "<b>t = 1</b>");
  executor.modules.set("live", rawEntry(cell, { cell_tick: () => result.ptr }));

  const frame = await executor.tickLive("live");

  assert.deepEqual(frame, { kind: 1, content: "<b>t = 1</b>" });
  assert.deepEqual(cell.log.deallocs, [
    [512, result.len],
    [result.ptr, LIVE_TICK_RESULT_SIZE],
  ]);
});

test("an sret tick allocates the return struct at the size of its own kind", async () => {
  for (const [method, size] of [
    ["tick", TICK_RESULT_SIZE],
    ["tickLive", LIVE_TICK_RESULT_SIZE],
  ]) {
    const { executor } = loadCore();
    const cell = makeMemory();
    let wroteTo = null;
    // One parameter: the caller allocates the struct and passes its address.
    function cell_tick(retptr) {
      wroteTo = retptr;
      cell.putU32s(retptr, [0, 0, 0, 0].slice(0, size / 4));
    }
    executor.modules.set("c", rawEntry(cell, { cell_tick }));

    await executor[method]("c");

    assert.deepEqual(cell.log.allocs, [size], `${method} allocates ${size} bytes`);
    assert.deepEqual(cell.log.deallocs, [[wroteTo, size]], `${method} frees what it allocated`);
  }
});

test("a trapping sret tick frees the struct it allocated", async () => {
  const { executor } = loadCore();
  const cell = makeMemory();
  executor.modules.set(
    "c",
    rawEntry(cell, {
      cell_tick(_retptr) {
        throw new Error("unreachable");
      },
    }),
  );

  await assert.rejects(executor.tickLive("c"), /WASM tick trapped/);
  assert.equal(cell.log.deallocs.length, 1);
  assert.equal(cell.log.deallocs[0][1], LIVE_TICK_RESULT_SIZE);
});

test("both tick kinds report a failed return-struct alloc the same way", async () => {
  for (const method of ["tick", "tickLive"]) {
    const { executor } = loadCore();
    const cell = makeMemory();
    cell.failAlloc = true;
    executor.modules.set("c", rawEntry(cell, { cell_tick(_retptr) {} }));
    await assert.rejects(executor[method]("c"), /ironpad_alloc failed for tick return struct/);
  }
});

test("bindgen ticks go through the glue's cell_tick and free with the right size", async () => {
  const { executor } = loadCore();
  const cell = makeMemory();
  const retptr = writeTickResult(cell);
  executor.modules.set("sim", bindgenEntry(cell, { cell_tick: async () => retptr }));

  const frame = await executor.tick("sim");
  assert.deepEqual(Array.from(frame.rgbBytes), [10, 20, 30]);
  assert.deepEqual(cell.log.deallocs.at(-1), [retptr, TICK_RESULT_SIZE]);

  const live = makeMemory();
  const result = writeLiveTickResult(live, "hi");
  // No glue wrapper: the raw export on `wasm` is the fallback.
  executor.modules.set("live", bindgenEntry(live, {}, { cell_tick: () => result.ptr }));

  assert.deepEqual(await executor.tickLive("live"), { kind: 1, content: "hi" });
  assert.deepEqual(live.log.deallocs.at(-1), [result.ptr, LIVE_TICK_RESULT_SIZE]);
});

test("ticks reject for an unloaded cell and for a null result pointer", async () => {
  const { executor } = loadCore();
  await assert.rejects(executor.tick("missing"), /Cell missing not loaded/);
  await assert.rejects(executor.tickLive("missing"), /Cell missing not loaded/);

  const cell = makeMemory();
  executor.modules.set("c", rawEntry(cell, { cell_tick: () => 0 }));
  await assert.rejects(executor.tick("c"), /cell_tick returned null/);
  executor.modules.set("b", bindgenEntry(cell, { cell_tick: async () => 0 }));
  await assert.rejects(executor.tickLive("b"), /cell_tick returned null/);
});

// ── UTF-8 codecs and CellResult (review js-5) ───────────────────────────────

test("the per-frame paths reuse the executor's codecs instead of building new ones", async () => {
  // Sim reads, host messages and LiveView frames run once per animation
  // frame, and each used to construct a fresh TextDecoder/TextEncoder.
  const { executor, stats } = loadCore();
  const loaded = stats.codecs;
  const cell = makeMemory({ shared: true });
  executor.modules.set("c1", rawEntry(cell, { cell_tick: () => writeLiveTickResult(cell, "x").ptr }));
  executor.simBusWrite("k", 1);
  const key = cell.putText(64, "k");
  const msg = cell.putText(128, '{"type":"sim_emit","key":"k","value":2}');

  for (let i = 0; i < 3; i++) {
    executor._simRead("c1", key.ptr, key.len);
    executor._simReadAll("c1", key.ptr, key.len);
    executor._dispatchHostMessage("c1", msg.ptr, msg.len);
    await executor.tickLive("c1");
  }

  assert.ok(stats.decodes >= 12, "the calls above did decode");
  assert.equal(stats.codecs, loaded, "and constructed no codec to do it");
});

test("execute decodes display text and type tag out of shared memory", async () => {
  // `_readCellResult` decodes through the executor's one shared decoder; the
  // copies it makes first are what keep that safe on a rayon cell's memory.
  const { executor } = loadCore();
  const cell = makeMemory({ shared: true });
  const display = cell.putText(600, '[{"Text":"42"}]');
  const tag = cell.putText(700, "i32");
  cell.put(800, [42, 0, 0, 0]);
  cell.putU32s(256, [800, 4, display.ptr, display.len, tag.ptr, tag.len]);
  executor.modules.set("c", rawEntry(cell, { cell_main: (_inPtr, _inLen) => 256 }));

  const result = await executor.execute("c", new Uint8Array(0));

  assert.equal(result.displayText, '[{"Text":"42"}]');
  assert.equal(result.typeTag, "i32");
  assert.deepEqual(Array.from(result.outputBytes), [42, 0, 0, 0]);
});
