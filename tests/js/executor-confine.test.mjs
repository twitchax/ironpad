/**
 * Cell-worker confinement tests (executor-confine.js, review bug-db-auth-1).
 *
 *   cargo make test-js
 *
 * Cells run in a same-origin worker, where the browser attaches the viewer's
 * session cookie to every request by default and the origin's IndexedDB and
 * Cache Storage are one property away. The prelude strips both. These run
 * the real prelude against a fake worker global shaped like Chromium's, where
 * `fetch`, `indexedDB` and `caches` live on `WorkerGlobalScope.prototype`
 * rather than on `self` (measured), so a patch applied to `self` alone would
 * leave the prototype's originals reachable.
 */

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";

import { loadCore, loadWorker } from "./executor-harness.mjs";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const CONFINE_SOURCE = readFileSync(
  path.join(HERE, "..", "..", "public", "executor-confine.js"),
  "utf8",
);

// ── Harness ─────────────────────────────────────────────────────────────────

/** A worker global laid out like Chromium's, with the prelude applied. */
function confinedWorker() {
  const requests = [];
  class WorkerGlobalScope {}
  WorkerGlobalScope.prototype.fetch = function (input) {
    requests.push({ thisArg: this, request: input });
    return Promise.resolve("response");
  };
  Object.defineProperty(WorkerGlobalScope.prototype, "indexedDB", {
    get: () => ({ open() {} }),
    configurable: true,
    enumerable: true,
  });
  Object.defineProperty(WorkerGlobalScope.prototype, "caches", {
    get: () => ({ open() {} }),
    configurable: true,
    enumerable: true,
  });
  class DedicatedWorkerGlobalScope extends WorkerGlobalScope {}
  const g = new DedicatedWorkerGlobalScope();
  g.self = g;
  g.XMLHttpRequest = function XMLHttpRequest() {};
  // eslint-disable-next-line no-new-func
  new Function("self", CONFINE_SOURCE)(g);
  return { g, requests, WorkerGlobalScope };
}

// ── Tests ───────────────────────────────────────────────────────────────────

test("every fetch leaves without credentials, whatever the caller asked for", async () => {
  const { g, requests } = confinedWorker();

  await g.fetch("https://ironpad.test/api/list_mutable_shares", {
    method: "POST",
    credentials: "include",
    headers: { "content-type": "application/x-www-form-urlencoded" },
    body: "",
  });
  await g.fetch(
    new Request("https://ironpad.test/mutable/abc", { credentials: "same-origin" }),
  );
  await g.fetch("https://api.example.com/data");

  assert.equal(requests.length, 3);
  for (const { request } of requests) {
    assert.ok(request instanceof Request, "the native fetch receives a Request");
    assert.equal(request.credentials, "omit");
  }
  // Only the credentials change: the rest of the request is the caller's.
  assert.equal(requests[0].request.method, "POST");
  assert.equal(
    requests[0].request.headers.get("content-type"),
    "application/x-www-form-urlencoded",
  );
  assert.equal(requests[1].request.url, "https://ironpad.test/mutable/abc");
  // fetch is called with the worker global as `this`, as a bare fetch() is.
  assert.equal(requests[2].thisArg, g);
});

test("the original fetch is unreachable through self or the prototype", async () => {
  const { g, requests, WorkerGlobalScope } = confinedWorker();

  // A cell that deletes the own property only uncovers the prototype's,
  // which is the same wrapper.
  const viaPrototype = WorkerGlobalScope.prototype.fetch;
  delete g.fetch;
  assert.equal(g.fetch, viaPrototype);
  await viaPrototype.call(g, "https://ironpad.test/x", { credentials: "include" });

  assert.equal(requests.at(-1).request.credentials, "omit");
});

test("a malformed request rejects like fetch does instead of throwing", async () => {
  const { g, requests } = confinedWorker();

  const result = g.fetch("not a url at all");

  assert.ok(result instanceof Promise);
  await assert.rejects(result, TypeError);
  assert.equal(requests.length, 0);
});

test("IndexedDB, Cache Storage and XMLHttpRequest are gone", () => {
  // XMLHttpRequest goes rather than being patched: a same-origin XHR sends
  // the cookie whatever `withCredentials` says.
  const { g } = confinedWorker();

  for (const name of ["indexedDB", "caches", "XMLHttpRequest"]) {
    assert.equal(name in g, false, `${name} must not be reachable`);
    assert.equal(g[name], undefined);
  }
});

test("the worker entry loads the prelude before anything else", () => {
  const { scripts } = loadWorker();

  assert.equal(scripts[0], "executor-worker.js");
  assert.equal(scripts[1], "executor-confine.js");
});

test("each rayon sub-worker imports the prelude before its module body", () => {
  // A static import is evaluated before the importing module's body runs,
  // so the pool's workers are confined before the cell's glue is imported.
  // The URL is absolute: a blob: worker has no base to resolve against.
  const { g } = loadCore();
  g.location.search = "?v=42";
  const glue = g.__IronpadExecutorGlue.rewriteBindgenGlue(
    "import { startWorkers } from './snippets/wasm-bindgen-rayon-abc/src/workerHelpers.js';\n",
    "cell-1",
    "self._ironpadExecutor",
  );

  // The first piece of the sub-worker's source, as the generated code spells it.
  const firstPiece = glue.match(/var subWorkerCode =\s*\n\s*(.+?) \+\n/);
  assert.ok(firstPiece, "the rewritten glue builds a sub-worker source");
  // eslint-disable-next-line no-new-func
  const firstLine = new Function(`return ${firstPiece[1]};`)();
  assert.equal(firstLine, 'import "https://ironpad.test/executor-confine.js?v=42";');
});
