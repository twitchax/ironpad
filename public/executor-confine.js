// Confinement prelude for the workers that run cell code (review
// bug-db-auth-1, Step 1). Loaded FIRST in executor-worker.js and imported
// first by every rayon sub-worker (executor-glue.js), so it runs before any
// cell does.
//
// Cells run in a same-origin Web Worker. Without this, every request a cell
// makes carries the viewer's session cookie (HttpOnly hides a cookie from
// script, not from the requests script makes), so a stranger's /shared
// notebook could call the session-gated server fns as whoever runs it, and
// the worker could read every local notebook out of the origin's IndexedDB.
// This strips both:
//
//   - fetch: every request leaves with `credentials: 'omit'`. That covers
//     ironpad_cell::http (reqwest), blocking::fetch_* and any eval'd fetch.
//     Cross-origin APIs keep working; they never received the cookie anyway.
//   - indexedDB and caches: removed.
//   - XMLHttpRequest: removed rather than patched, because a same-origin XHR
//     sends the cookie whatever `withCredentials` says.
//
// In Chromium `fetch`, `indexedDB` and `caches` live on
// WorkerGlobalScope.prototype, not on `self`, so every object on the global's
// prototype chain that owns one is patched; patching `self` alone would leave
// the original one `delete self.fetch` away.
//
// This is a speed bump, NOT a boundary. Code that runs in a scope this
// prelude never reaches is unconfined: chiefly a cell's own
// `new Worker(blobUrl)`, a fresh global with the original fetch and
// IndexedDB. The boundary is running cells from a separate origin, where the
// session cookie and IndexedDB do not exist to be reached.

(function () {
  "use strict";

  var scope = self;

  // Apply `fn` to every object on the global's prototype chain that owns
  // `name` (Chromium: the prototype; elsewhere possibly the global itself).
  function eachOwner(name, fn) {
    for (var o = scope; o; o = Object.getPrototypeOf(o)) {
      if (Object.prototype.hasOwnProperty.call(o, name)) fn(o);
    }
  }

  var nativeFetch = scope.fetch;
  if (typeof nativeFetch === "function") {
    var confinedFetch = function fetch(input, init) {
      var request;
      try {
        request = new Request(
          input,
          Object.assign({}, init, { credentials: "omit" })
        );
      } catch (e) {
        // fetch() rejects on a malformed request rather than throwing.
        return Promise.reject(e);
      }
      return nativeFetch.call(scope, request);
    };
    eachOwner("fetch", function (o) {
      Object.defineProperty(o, "fetch", {
        value: confinedFetch,
        writable: true,
        enumerable: true,
        configurable: true,
      });
    });
    scope.fetch = confinedFetch;
  }

  ["indexedDB", "caches", "XMLHttpRequest"].forEach(function (name) {
    eachOwner(name, function (o) {
      delete o[name];
    });
  });
})();
