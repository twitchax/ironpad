// Main-thread postMessage bridge for ironpad cell execution.
// Exposes the same window.IronpadExecutor API as executor.js, but delegates
// all WASM compilation and execution to a Web Worker via postMessage.
//
// When worker execution fails (e.g. WASM panic from DOM-dependent libraries
// like plotters), the bridge automatically retries on the main thread using
// a lazily-loaded CellExecutor.  Results from this fallback path include a
// `fallback: true` field so the UI can show a diagnostic indicator.
//
// See PRD-0013 for architecture.  The bridge maintains a local isLoaded cache
// so the synchronous `isLoaded(cellId, hash)` call works without a round-trip
// to the Worker.

(function () {
  "use strict";

  // The shell loads this script with a ?v={release} cache-buster (versioned()
  // in lib.rs). Propagate that version to every script this bridge loads
  // lazily (worker entry, main-thread fallback executor) so the whole
  // executor chain busts caches together — the no-cache middleware is the
  // safety net, this makes stale mixes structurally impossible.
  var SCRIPT_VERSION = (function () {
    try {
      var src = document.currentScript && document.currentScript.src;
      var m = src && src.match(/[?&]v=([^&]+)/);
      return m ? m[1] : null;
    } catch (e) {
      return null;
    }
  })();

  function versionedPath(path) {
    return SCRIPT_VERSION ? path + "?v=" + SCRIPT_VERSION : path;
  }

  // ── BridgeExecutor ────────────────────────────────────────────────────────

  // Standalone copy of CellExecutor.updateSimBus (executor-core.js). The bridge
  // runs on the main thread, where executor-core.js is NOT loaded at init time
  // (it's only injected lazily for the fallback path), so the static helper
  // isn't reachable here — keep this copy in sync with the core one, including
  // the ring cap.
  var SIM_BUS_RING_CAP = 1000; // mirrors SIM_BUS_RING_CAP in executor-core.js
  function _updateSimBus(bus, key, json) {
    var entry = bus.get(key);
    if (!entry) {
      entry = { latest: null, ring: [] };
      bus.set(key, entry);
    }
    entry.latest = json;
    entry.ring.push(json);
    if (entry.ring.length > SIM_BUS_RING_CAP) entry.ring.shift();
  }

  // A worker that errors within this window of being spawned counts as a
  // rapid failure; after WORKER_MAX_RAPID_FAILURES consecutive ones the
  // bridge stops respawning and stays on the main-thread fallback. Without
  // the cap, a persistently failing worker script (offline tab, 404 after a
  // bad deploy) turned onerror -> spawn -> onerror into an unbounded tight
  // loop burning CPU and network for the life of the tab.
  var WORKER_RAPID_FAIL_MS = 5000;
  var WORKER_MAX_RAPID_FAILURES = 3;

  function BridgeExecutor() {
    this._nextId = 1;
    this._pending = new Map();       // reqId -> { resolve, reject }
    this._loadedCache = new Map();   // cellId -> hash
    this._blobCache = new Map();     // cellId -> { hash, wasmBytes, jsGlue }
    this._messageHandlers = {};      // type -> handler(msg, cellId)
    this._simBus = new Map();        // key -> { latest: string|null, ring: string[] }
    this._worker = null;
    this._workerSpawnedAt = 0;       // for the rapid-failure window
    this._workerRapidFailures = 0;   // consecutive rapid failures
    this._mainExecutor = null;       // Lazy main-thread CellExecutor for fallback
    this._mainExecutorPromise = null; // In-flight fallback load (memoized)

    this._spawnWorker();
  }

  // ── Worker lifecycle ──────────────────────────────────────────────────────

  BridgeExecutor.prototype._spawnWorker = function () {
    var self = this;
    this._workerSpawnedAt = Date.now();
    this._worker = new Worker(versionedPath("/executor-worker.js"));

    this._worker.onmessage = function (e) {
      self._onWorkerMessage(e.data);
    };

    this._worker.onerror = function (e) {
      console.error("ironpad: worker error:", e.message);
      // A worker-level failure (script-load error, uncaught exception) leaves
      // every in-flight request unsettled — reject them so cells don't hang on
      // "Compiling/Running" forever.
      var err = new Error(
        "Worker crashed: " + (e.message || "unknown worker error")
      );
      self._pending.forEach(function (entry) {
        entry.reject(err);
      });
      self._pending.clear();
      self._loadedCache.clear();

      // Terminate before respawning: per the HTML spec an uncaught error
      // does NOT stop a worker, so skipping this abandoned a still-running
      // worker (loaded WASM, rayon sub-pool, any runaway cell) that kept
      // posting stale messages into the new worker's handlers.
      self._worker.terminate();
      self._worker = null;

      var lifetime = Date.now() - self._workerSpawnedAt;
      self._workerRapidFailures =
        lifetime < WORKER_RAPID_FAIL_MS ? self._workerRapidFailures + 1 : 1;
      if (self._workerRapidFailures >= WORKER_MAX_RAPID_FAILURES) {
        console.error(
          "ironpad: worker failed " + self._workerRapidFailures +
          " times in quick succession; not respawning. Cells will run on " +
          "the main-thread fallback executor."
        );
        return;
      }
      self._spawnWorker();
    };
  };

  BridgeExecutor.prototype._onWorkerMessage = function (msg) {
    if (msg.type === "result" || msg.type === "error") {
      var entry = this._pending.get(msg.id);
      if (entry) {
        this._pending.delete(msg.id);
        if (msg.type === "result") {
          entry.resolve(msg.value);
        } else {
          entry.reject(new Error(msg.error));
        }
      }
      return;
    }

    if (msg.type === "hostMessage") {
      this._dispatchHostMessage(msg.cellId, msg.messageJson);
      return;
    }
  };

  // ── Main-thread fallback executor ─────────────────────────────────────────

  BridgeExecutor.prototype._ensureMainExecutor = function () {
    if (this._mainExecutor) return Promise.resolve(this._mainExecutor);
    // Memoize the in-flight load so two concurrent fallbacks don't both inject
    // the scripts and race the window.IronpadExecutor capture/restore dance
    // (which could leave the global pointing at a fallback with no terminate()).
    if (this._mainExecutorPromise) return this._mainExecutorPromise;

    var self = this;
    this._mainExecutorPromise = new Promise(function (resolve, reject) {
      // Load order matters: executor-core.js reads the Gpu/Glue namespaces
      // its companions register (PRD-0055 T-002), and executor.js reads
      // self.__IronpadExecutorCore — so the chain is gpu, glue, core,
      // executor.js, strictly sequential.
      var chain = [
        "/executor-gpu.js",
        "/executor-glue.js",
        "/executor-core.js",
      ];
      function loadNext() {
        var path = chain.shift();
        if (path === undefined) {
          loadExecutorJs();
          return;
        }
        var script = document.createElement("script");
        script.src = versionedPath(path);
        script.onload = loadNext;
        script.onerror = function () {
          self._mainExecutorPromise = null; // allow a later retry
          reject(new Error("Failed to load " + path));
        };
        document.head.appendChild(script);
      }
      function loadExecutorJs() {
        var script = document.createElement("script");
        // executor.js sets window.IronpadExecutor, but we already own that
        // global.  We load it, grab the CellExecutor from the temporary
        // overwrite, then restore our bridge instance.
        var prev = window.IronpadExecutor;
        script.src = versionedPath("/executor.js");
        script.onload = function () {
          self._mainExecutor = window.IronpadExecutor;
          window.IronpadExecutor = prev;
          // Register host message handlers on the main-thread executor so
          // progress bars etc. work in fallback mode too.
          for (var type in self._messageHandlers) {
            self._mainExecutor.onHostMessage(type, self._messageHandlers[type]);
          }
          resolve(self._mainExecutor);
        };
        script.onerror = function () {
          self._mainExecutorPromise = null; // allow a later retry
          reject(new Error("Failed to load fallback executor"));
        };
        document.head.appendChild(script);
      }
      loadNext();
    });
    return this._mainExecutorPromise;
  };

  // ── Request/response helpers ──────────────────────────────────────────────

  BridgeExecutor.prototype._postRequest = function (message) {
    var self = this;
    var id = this._nextId++;
    message.id = id;

    // No worker (respawn cap reached): reject so callers with a main-thread
    // fallback (execute and friends) take it immediately.
    if (!this._worker) {
      return Promise.reject(
        new Error("Worker unavailable (respawn limit reached)")
      );
    }

    return new Promise(function (resolve, reject) {
      self._pending.set(id, { resolve: resolve, reject: reject });
      self._worker.postMessage(message);
    });
  };

  // ── Public API (matches executor.js surface) ──────────────────────────────

  /// Load a compiled WASM blob for a cell.
  ///
  /// Sends the blob data to the Worker for loading.  On success, updates the
  /// local isLoaded cache so `isLoaded()` can answer synchronously.
  BridgeExecutor.prototype.loadBlob = function (cellId, hash, wasmBytes, jsGlue) {
    var self = this;

    // Cache blob data for potential main-thread fallback.
    this._blobCache.set(cellId, {
      hash: hash,
      wasmBytes: wasmBytes,
      jsGlue: jsGlue || null,
    });

    // Fast path: already loaded with the same hash.
    var cached = this._loadedCache.get(cellId);
    if (cached === hash) {
      return Promise.resolve();
    }

    return this._postRequest({
      type: "loadBlob",
      cellId: cellId,
      hash: hash,
      wasmBytes: wasmBytes,
      jsGlue: jsGlue || null,
    }).then(function () {
      self._loadedCache.set(cellId, hash);
    }).catch(async function (workerError) {
      // A deliberate cancellation must surface as-is (see execute()).
      if (workerError && workerError.name === "AbortError") {
        throw workerError;
      }
      // The worker is unavailable (e.g. the respawn cap tripped after
      // repeated crashes). Without a fallback here every run would fail with
      // "Worker unavailable" BEFORE execute()'s own main-thread fallback
      // could fire (the Rust flow loads then executes). Load on the
      // main-thread executor so the cell still runs; execute() then runs it
      // there too. Do NOT touch _loadedCache — that tracks the WORKER's
      // loaded state, and this blob is loaded on the main executor.
      console.warn(
        "ironpad: worker loadBlob failed for " + cellId +
        ", loading on main thread. Worker error: " + workerError.message
      );
      var exec = await self._ensureMainExecutor();
      var blob = self._blobCache.get(cellId);
      if (!blob) throw workerError;
      if (!exec.isLoaded(cellId, blob.hash)) {
        await exec.loadBlob(cellId, blob.hash, blob.wasmBytes, blob.jsGlue);
      }
    });
  };

  /// Execute a loaded cell with the given input bytes.
  ///
  /// Returns Promise<{ outputBytes, displayText, typeTag, fallback? }>.
  /// If the worker execution fails, automatically retries on the main thread
  /// and marks the result with `fallback: true`.
  BridgeExecutor.prototype.execute = function (cellId, inputBytes) {
    var self = this;

    return this._postRequest({
      type: "execute",
      cellId: cellId,
      inputBytes: inputBytes,
    }).catch(async function (workerError) {
      // A deliberate cancellation (terminate() rejects pending requests with an
      // AbortError) must NOT fall back to the main thread — re-running a runaway
      // cell there would freeze the tab. Rethrow so the caller sees the cancel.
      if (workerError && workerError.name === "AbortError") {
        throw workerError;
      }
      // Worker execution failed — fall back to main-thread execution.
      console.warn(
        "ironpad: worker execution failed for " + cellId +
        ", retrying on main thread. Worker error: " + workerError.message
      );

      var exec = await self._ensureMainExecutor();
      var blob = self._blobCache.get(cellId);

      if (!blob) throw workerError; // No cached blob — can't fall back.

      if (!exec.isLoaded(cellId, blob.hash)) {
        await exec.loadBlob(cellId, blob.hash, blob.wasmBytes, blob.jsGlue);
      }

      var result = await exec.execute(cellId, inputBytes);
      result.fallback = true;
      return result;
    });
  };

  /// Execute a single tick on a loaded simulation cell.
  ///
  /// Returns Promise<{ width, height, rgbBytes, fallback? }>.
  /// If the worker tick fails, automatically retries on the main thread
  /// and marks the result with `fallback: true`.
  BridgeExecutor.prototype.tick = function (cellId) {
    var self = this;

    return this._postRequest({
      type: "tick",
      cellId: cellId,
    }).catch(async function (workerError) {
      // Deliberate cancellation (AbortError) must not fall back — see execute().
      if (workerError && workerError.name === "AbortError") {
        throw workerError;
      }
      // Worker tick failed — fall back to main-thread execution.
      console.warn(
        "ironpad: worker tick failed for " + cellId +
        ", retrying on main thread. Worker error: " + workerError.message
      );

      var exec = await self._ensureMainExecutor();
      var blob = self._blobCache.get(cellId);

      if (!blob) throw workerError; // No cached blob — can't fall back.

      if (!exec.isLoaded(cellId, blob.hash)) {
        await exec.loadBlob(cellId, blob.hash, blob.wasmBytes, blob.jsGlue);
      }

      if (!exec.tick) throw workerError; // Main-thread executor doesn't support tick.

      var result = await exec.tick(cellId);
      result.fallback = true;
      return result;
    });
  };

  /// Execute a single tick on a loaded LiveView cell.
  ///
  /// Returns Promise<{ kind, content, fallback? }>.
  /// If the worker tick fails, automatically retries on the main thread.
  BridgeExecutor.prototype.tickLive = function (cellId) {
    var self = this;

    return this._postRequest({
      type: "tick_live",
      cellId: cellId,
    }).catch(async function (workerError) {
      // Deliberate cancellation (AbortError) must not fall back — see execute().
      if (workerError && workerError.name === "AbortError") {
        throw workerError;
      }
      console.warn(
        "ironpad: worker tickLive failed for " + cellId +
        ", retrying on main thread. Worker error: " + workerError.message
      );

      var exec = await self._ensureMainExecutor();
      var blob = self._blobCache.get(cellId);

      if (!blob) throw workerError;

      if (!exec.isLoaded(cellId, blob.hash)) {
        await exec.loadBlob(cellId, blob.hash, blob.wasmBytes, blob.jsGlue);
      }

      if (!exec.tickLive) throw workerError;

      var result = await exec.tickLive(cellId);
      result.fallback = true;
      return result;
    });
  };

  /// Remove a loaded cell module.  Fire-and-forget (no response expected).
  BridgeExecutor.prototype.unload = function (cellId) {
    this._loadedCache.delete(cellId);
    this._blobCache.delete(cellId);
    if (this._worker) {
      this._worker.postMessage({ type: "unload", cellId: cellId });
    }

    // Also unload from main-thread executor if present.
    if (this._mainExecutor) {
      this._mainExecutor.unload(cellId);
    }
  };

  /// Check whether a cell has a module loaded with the given hash.
  ///
  /// Synchronous — answered from the local cache maintained by loadBlob.
  BridgeExecutor.prototype.isLoaded = function (cellId, hash) {
    return this._loadedCache.get(cellId) === hash;
  };

  // ── Host message infrastructure ───────────────────────────────────────────

  /// Register a handler for a specific host message type.
  BridgeExecutor.prototype.onHostMessage = function (type, handler) {
    this._messageHandlers[type] = handler;

    // Also register on the main-thread executor if already loaded.
    if (this._mainExecutor) {
      this._mainExecutor.onHostMessage(type, handler);
    }
  };

  /// Dispatch a host message received from the Worker.
  ///
  /// The Worker reads the raw WASM memory and sends the JSON text to the main
  /// thread.  We parse it here and dispatch to registered handlers.
  BridgeExecutor.prototype._dispatchHostMessage = function (cellId, messageJson) {
    try {
      var msg = JSON.parse(messageJson);
      var handler = this._messageHandlers[msg.type];
      if (handler) {
        handler(msg, cellId);
      }
    } catch (e) {
      console.warn("ironpad: failed to parse host message:", e);
    }
  };

  // ── Sim bus ───────────────────────────────────────────────────────────────

  /// Write a value to the sim bus from the main thread (e.g. from a slider).
  ///
  /// Updates the bridge's own _simBus, forwards to the Worker so simulation
  /// cells running there can read the value via ironpad_sim_read, and also
  /// updates the main-thread fallback executor's bus if present.
  BridgeExecutor.prototype.simBusWrite = function (key, value) {
    var json = JSON.stringify(value);

    // Update bridge bus (main-thread side).
    _updateSimBus(this._simBus, key, json);

    // Forward to worker bus.
    if (this._worker) {
      this._worker.postMessage({ type: "sim_bus_write", key: key, json: json });
    }

    // Mirror to main-thread fallback executor bus if loaded.
    if (this._mainExecutor) {
      _updateSimBus(this._mainExecutor._simBus, key, json);
    }
  };

  /// Read the latest value from the bridge's sim bus (convenience for debugging).
  BridgeExecutor.prototype.simBusRead = function (key) {
    var entry = this._simBus.get(key);
    return entry ? JSON.parse(entry.latest) : null;
  };

  // ── Termination ───────────────────────────────────────────────────────────

  /// Kill the running Worker and respawn a fresh one.
  ///
  /// All pending Promises are rejected with an AbortError.  The isLoaded
  /// cache is cleared — Rust will re-trigger loadBlob when needed.
  BridgeExecutor.prototype.terminate = function () {
    // Kill the Worker immediately (it may already be gone if the respawn
    // cap was reached).
    if (this._worker) {
      this._worker.terminate();
      this._worker = null;
    }

    // Reject all pending requests.
    this._pending.forEach(function (entry) {
      var err = new DOMException("Worker terminated", "AbortError");
      entry.reject(err);
    });
    this._pending.clear();

    // Reset state and spawn a fresh Worker. An explicit user-driven
    // terminate also resets the rapid-failure counter — it is a deliberate
    // fresh start, not another crash in the loop.
    // NOTE: _blobCache is intentionally preserved for fallback.
    this._loadedCache.clear();
    this._workerRapidFailures = 0;
    this._spawnWorker();
  };

  // ── Expose as a global singleton ──────────────────────────────────────────

  var executor = new BridgeExecutor();

  // ── Built-in host message handlers ────────────────────────────────────────
  //
  // NOTE: These built-in handlers are intentionally duplicated in executor.js
  // (the main-thread fallback executor). The bridge can't load executor-core.js
  // on the main thread at init time, so there is no shared home for them — keep
  // the two copies in sync.

  executor.onHostMessage("progress_update", function (msg, _cellId) {
    var el = document.querySelector('[data-progress-id="' + msg.id + '"]');
    if (!el) return;

    // Harden the host-message boundary: a non-numeric value would render
    // "NaN%" and `width: NaN%`. Fall back to 0 when it isn't a finite number.
    var value = Number.isFinite(msg.value) ? msg.value : 0;

    var fill = el.querySelector(".ironpad-progress-fill");
    if (fill) {
      var pct = Math.min(100, Math.max(0, value));
      fill.style.width = pct + "%";
    }

    var label = el.querySelector(".ironpad-progress-value");
    if (label) {
      label.textContent = Math.round(value) + "%";
    }
  });

  executor.onHostMessage("sim_emit", function (msg, _cellId) {
    _updateSimBus(executor._simBus, msg.key, JSON.stringify(msg.value));
  });

  window.IronpadExecutor = executor;
})();
