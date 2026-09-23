/**
 * Shared harness for the executor unit tests (executor-core.js, the worker
 * entry, and the bridge's fallback onto the core).
 *
 * It loads the real executor scripts from `public/` against a fake global,
 * then hands tests FAKE loaded entries: a `{ type: "raw", instance: {
 * exports } }` or `{ type: "bindgen", module, wasm }` record for
 * `executor.modules`, whose memory is a real `WebAssembly.Memory` and whose
 * `ironpad_alloc`/`cell_tick` are plain JS. No cell is compiled and no
 * browser runs.
 *
 * The scripts run with `TextDecoder` bound to `BrowserTextDecoder`, which
 * refuses views over a SharedArrayBuffer the way browsers do. Node's own
 * decoder accepts them, so without it a live-view decode of a rayon cell's
 * shared memory (a trap inside a WASM import, in a browser) would pass here.
 * Both codecs also count their constructions, which the executor makes once
 * per script load rather than once per per-frame call.
 *
 * Not a `*.test.mjs` file, so `cargo make test-js` does not run it directly.
 */

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const PUBLIC = path.join(HERE, "..", "..", "public");

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
export function loadCore() {
  const env = makeGlobal();
  env.run("executor-gpu.js");
  env.run("executor-glue.js");
  env.run("executor-core.js");
  const executor = new env.g.__IronpadExecutorCore.CellExecutor("self._ironpadExecutor");
  return { ...env, executor };
}

/** The worker entry, loaded through its own importScripts chain. */
export function loadWorker() {
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
export function makeMemory({ shared = false } = {}) {
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

export function rawEntry(cell, extraExports = {}) {
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

export function bindgenEntry(cell, moduleExports = {}, extraWasm = {}) {
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
export function writeTickResult(cell) {
  cell.put(512, [10, 20, 30]);
  return cell.putU32s(256, [512, 3, 1, 1]);
}

/** A LiveTickResult at 256 (kind 1 = Html) pointing at UTF-8 content at 512. */
export function writeLiveTickResult(cell, content) {
  const { len } = cell.putText(512, content);
  cell.putU32s(256, [1, 512, len]);
  return { ptr: 256, len };
}
