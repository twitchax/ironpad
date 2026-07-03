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

  // ── BridgeExecutor ────────────────────────────────────────────────────────

  // Local sim bus update helper — mirrors CellExecutor.updateSimBus from
  // executor-core.js (which is not loaded when the bridge initializes).
  function _updateSimBus(bus, key, json) {
    var entry = bus.get(key);
    if (!entry) {
      entry = { latest: null, ring: [] };
      bus.set(key, entry);
    }
    entry.latest = json;
    entry.ring.push(json);
    if (entry.ring.length > 1000) entry.ring.shift();
  }

  function BridgeExecutor() {
    this._nextId = 1;
    this._pending = new Map();       // reqId -> { resolve, reject }
    this._loadedCache = new Map();   // cellId -> hash
    this._blobCache = new Map();     // cellId -> { hash, wasmBytes, jsGlue }
    this._messageHandlers = {};      // type -> handler(msg, cellId)
    this._simBus = new Map();        // key -> { latest: string|null, ring: string[] }
    this._worker = null;
    this._mainExecutor = null;       // Lazy main-thread CellExecutor for fallback
    this._mainExecutorPromise = null; // In-flight fallback load (memoized)

    this._spawnWorker();
  }

  // ── Worker lifecycle ──────────────────────────────────────────────────────

  BridgeExecutor.prototype._spawnWorker = function () {
    var self = this;
    this._worker = new Worker("/executor-worker.js");

    this._worker.onmessage = function (e) {
      self._onWorkerMessage(e.data);
    };

    this._worker.onerror = function (e) {
      console.error("ironpad: worker error:", e.message);
      // A worker-level failure (script-load error, uncaught exception) leaves
      // every in-flight request unsettled — reject them so cells don't hang on
      // "Compiling/Running" forever, then respawn a fresh worker for later use.
      var err = new Error(
        "Worker crashed: " + (e.message || "unknown worker error")
      );
      self._pending.forEach(function (entry) {
        entry.reject(err);
      });
      self._pending.clear();
      self._loadedCache.clear();
      self._spawnWorker();
    };
  };

  BridgeExecutor.prototype._onWorkerMessage = function (msg) {
    if (msg.type === "result") {
      var entry = this._pending.get(msg.id);
      if (entry) {
        this._pending.delete(msg.id);
        entry.resolve(msg.value);
      }
      return;
    }

    if (msg.type === "error") {
      var entry = this._pending.get(msg.id);
      if (entry) {
        this._pending.delete(msg.id);
        entry.reject(new Error(msg.error));
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
      // executor.js depends on executor-core.js (reads self.__IronpadExecutorCore),
      // so we must load core first.
      var coreScript = document.createElement("script");
      coreScript.src = "/executor-core.js";
      coreScript.onload = function () {
        var script = document.createElement("script");
        // executor.js sets window.IronpadExecutor, but we already own that
        // global.  We load it, grab the CellExecutor from the temporary
        // overwrite, then restore our bridge instance.
        var prev = window.IronpadExecutor;
        script.src = "/executor.js";
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
      };
      coreScript.onerror = function () {
        self._mainExecutorPromise = null; // allow a later retry
        reject(new Error("Failed to load executor-core.js"));
      };
      document.head.appendChild(coreScript);
    });
    return this._mainExecutorPromise;
  };

  BridgeExecutor.prototype._executeOnMainThread = async function (cellId) {
    var exec = await this._ensureMainExecutor();
    var blob = this._blobCache.get(cellId);

    if (!blob) throw new Error("No cached blob for cell " + cellId + " — cannot fall back to main thread");

    // Load the blob into the main-thread executor if not already loaded.
    if (!exec.isLoaded(cellId, blob.hash)) {
      await exec.loadBlob(cellId, blob.hash, blob.wasmBytes, blob.jsGlue);
    }

    // Re-gather input bytes — execute with empty input since we don't cache
    // input in the bridge.  The caller will pass inputBytes via the wrapper.
    return exec;
  };

  // ── Request/response helpers ──────────────────────────────────────────────

  BridgeExecutor.prototype._postRequest = function (message) {
    var self = this;
    var id = this._nextId++;
    message.id = id;

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
    this._worker.postMessage({ type: "unload", cellId: cellId });

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
    this._worker.postMessage({ type: "sim_bus_write", key: key, json: json });

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
    // Kill the Worker immediately.
    this._worker.terminate();

    // Reject all pending requests.
    this._pending.forEach(function (entry) {
      var err = new DOMException("Worker terminated", "AbortError");
      entry.reject(err);
    });
    this._pending.clear();

    // Reset state and spawn a fresh Worker.
    // NOTE: _blobCache is intentionally preserved for fallback.
    this._loadedCache.clear();
    this._spawnWorker();
  };

  // ── Expose as a global singleton ──────────────────────────────────────────

  var executor = new BridgeExecutor();

  // ── Built-in host message handlers ────────────────────────────────────────

  executor.onHostMessage("progress_update", function (msg, _cellId) {
    var el = document.querySelector('[data-progress-id="' + msg.id + '"]');
    if (!el) return;

    var fill = el.querySelector(".ironpad-progress-fill");
    if (fill) {
      var pct = Math.min(100, Math.max(0, msg.value));
      fill.style.width = pct + "%";
    }

    var label = el.querySelector(".ironpad-progress-value");
    if (label) {
      label.textContent = Math.round(msg.value) + "%";
    }
  });

  executor.onHostMessage("sim_emit", function (msg, _cellId) {
    _updateSimBus(executor._simBus, msg.key, JSON.stringify(msg.value));
  });

  window.IronpadExecutor = executor;
})();
