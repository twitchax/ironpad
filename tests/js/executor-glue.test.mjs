/**
 * The generated rayon `startWorkers` and its sub-worker text
 * (executor-glue.js).
 *
 *   cargo make test-js
 *
 * rewriteBindgenGlue writes JavaScript as strings, and the sub-worker's
 * source is a string inside those strings, so a quoting slip only shows when
 * a pool actually starts. These evaluate the generated text against fake
 * Worker/Blob/URL globals: the pool startWorkers spawns, and the code each
 * sub-worker runs.
 */

import { test } from "node:test";
import assert from "node:assert/strict";

import { loadCore, within } from "./executor-harness.mjs";

const RAYON_IMPORT =
  "import { startWorkers } from './snippets/wasm-bindgen-rayon-3e04391371ad0a8e/src/workerHelpers.js';";

/**
 * The rewritten glue's `startWorkers` for `cellId`, evaluated against fake
 * globals. Each spawned FakeWorker records its source text and what it is
 * posted, and answers the init message with `wasm_bindgen_worker_ready`.
 */
function startWorkersFor(cellId, workerGlobal) {
  const { g } = loadCore();
  const glue = g.__IronpadExecutorGlue.rewriteBindgenGlue(RAYON_IMPORT, cellId, "self._ironpadExecutor");

  const blobs = new Map();
  let nextUrl = 0;
  const FakeURL = {
    createObjectURL(blob) {
      const url = `blob:fake/${(nextUrl += 1)}`;
      blobs.set(url, blob.text);
      return url;
    },
    revokeObjectURL() {},
  };
  class FakeBlob {
    constructor(parts) {
      this.text = parts.join("");
    }
  }
  const workers = [];
  class FakeWorker {
    constructor(url) {
      this.source = blobs.get(url);
      this.posted = [];
      this.listeners = { message: new Set(), error: new Set() };
      this.terminated = false;
      workers.push(this);
    }
    addEventListener(type, fn) {
      this.listeners[type].add(fn);
    }
    removeEventListener(type, fn) {
      this.listeners[type].delete(fn);
    }
    postMessage(msg) {
      this.posted.push(msg);
      queueMicrotask(() => {
        for (const fn of [...this.listeners.message]) fn({ data: { type: "wasm_bindgen_worker_ready" } });
      });
    }
    terminate() {
      this.terminated = true;
    }
  }

  // eslint-disable-next-line no-new-func
  const startWorkers = new Function("self", "Worker", "Blob", "URL", `${glue}\nreturn startWorkers;`)(
    workerGlobal,
    FakeWorker,
    FakeBlob,
    FakeURL,
  );
  return { startWorkers, workers };
}

function builder(numThreads = 2) {
  return {
    numThreads: () => numThreads,
    receiver: () => 42,
    build() {
      this.built = true;
    },
  };
}

/**
 * Run one sub-worker's source with `init` as its init message. The glue it
 * imports comes from `glueSource`, served as a data: URL so Node can import
 * it; the sub-worker's console is `subConsole`, which the glue reaches
 * through `globalThis.__ironpadTestConsole`.
 */
async function runSubWorker(source, init, glueSource, subConsole) {
  const posts = [];
  class FakeBroadcastChannel {
    constructor(name) {
      this.name = name;
    }
    postMessage(msg) {
      posts.push({ channel: this.name, msg });
    }
  }
  const FakeURL = {
    createObjectURL: (blob) => `data:text/javascript;base64,${Buffer.from(blob.text).toString("base64")}`,
    revokeObjectURL() {},
  };
  class FakeBlob {
    constructor(parts) {
      this.text = parts.join("");
    }
  }
  const sub = { posted: [], postMessage: (m) => sub.posted.push(m) };
  globalThis.__ironpadTestConsole = subConsole;
  try {
    // eslint-disable-next-line no-new-func
    new Function("self", "console", "BroadcastChannel", "URL", "Blob", source)(
      sub,
      subConsole,
      FakeBroadcastChannel,
      FakeURL,
      FakeBlob,
    );
    await within(sub.onmessage({ data: { ...init, glueCode: glueSource } }));
  } finally {
    delete globalThis.__ironpadTestConsole;
  }
  return { posts, sub };
}

// ── Rayon traps (review bug-cell-14) ────────────────────────────────────────

test("startWorkers hands every pool thread the bridge's trap channel", async () => {
  const { startWorkers, workers } = startWorkersFor("cell-a", { __ironpadRayonTrapName: "trap-123" });
  const b = builder(2);

  await within(startWorkers({}, {}, b));

  assert.equal(b.built, true);
  assert.equal(workers.length, 2);
  for (const w of workers) {
    assert.equal(w.posted[0].trapChannel, "trap-123");
    assert.equal(w.posted[0].receiver, 42);
  }
});

test("a pool thread that traps reports its panic text on the trap channel", async () => {
  // rayon's catch_unwind does nothing under panic=abort, so a panic in a
  // parallel closure traps out of wbg_rayon_start_worker. The executor
  // worker is parked on the job's latch and cannot hear it, so the
  // sub-worker tells the page directly.
  const { startWorkers, workers } = startWorkersFor("cell-a", { __ironpadRayonTrapName: "trap-123" });
  await within(startWorkers({}, {}, builder(1)));
  const [pool] = workers;

  const logged = [];
  const subConsole = { error: (...args) => logged.push(args.join(" ")) };
  const glueSource = `
    export default async function init() {}
    export function wbg_rayon_start_worker(receiver) {
      globalThis.__ironpadTestConsole.error(
        "panicked at src/lib.rs:2:3:\\nbad row\\n\\nStack:\\n\\nError\\n    at wasm-function[9]"
      );
      throw new WebAssembly.RuntimeError("unreachable");
    }
  `;
  const { posts, sub } = await runSubWorker(pool.source, pool.posted[0], glueSource, subConsole);

  assert.deepEqual(sub.posted, [{ type: "wasm_bindgen_worker_ready" }]);
  assert.deepEqual(posts, [
    { channel: "trap-123", msg: { message: "panicked at src/lib.rs:2:3:\nbad row" } },
  ]);
  assert.equal(logged.length, 1, "the panic still reaches the console");
});

test("a pool thread with no captured panic reports the trap itself", async () => {
  const { startWorkers, workers } = startWorkersFor("cell-a", { __ironpadRayonTrapName: "trap-9" });
  await within(startWorkers({}, {}, builder(1)));
  const [pool] = workers;

  const glueSource = `
    export default async function init() {}
    export function wbg_rayon_start_worker() { throw new WebAssembly.RuntimeError("unreachable"); }
  `;
  const { posts } = await runSubWorker(pool.source, pool.posted[0], glueSource, { error() {} });

  assert.equal(posts.length, 1);
  assert.match(posts[0].msg.message, /RuntimeError: unreachable/);
});

// ── One pool per rayon cell (review bug-viewer-1) ───────────────────────────

test("loading a second rayon cell leaves the first cell's pool running", async () => {
  // Each rayon cell is its own module with its own registry. One shared pool
  // slot let cell B's load terminate cell A's threads while A stayed loaded,
  // so A's next run blocked on a latch no thread would ever set.
  const workerGlobal = {};
  const a = startWorkersFor("cell-a", workerGlobal);
  const b = startWorkersFor("cell-b", workerGlobal);

  await within(a.startWorkers({}, {}, builder(2)));
  await within(b.startWorkers({}, {}, builder(2)));

  assert.deepEqual(a.workers.map((w) => w.terminated), [false, false]);
  assert.deepEqual(Object.keys(workerGlobal.__ironpadRayonWorkers).sort(), ["cell-a", "cell-b"]);
});

test("reloading one rayon cell replaces only that cell's pool", async () => {
  const workerGlobal = {};
  const a1 = startWorkersFor("cell-a", workerGlobal);
  const b = startWorkersFor("cell-b", workerGlobal);
  const a2 = startWorkersFor("cell-a", workerGlobal);

  await within(a1.startWorkers({}, {}, builder(1)));
  await within(b.startWorkers({}, {}, builder(1)));
  await within(a2.startWorkers({}, {}, builder(1)));

  assert.equal(a1.workers[0].terminated, true, "the reloaded cell's old pool is released");
  assert.equal(b.workers[0].terminated, false);
  assert.equal(workerGlobal.__ironpadRayonWorkers["cell-a"][0], a2.workers[0]);
});
