// Main-thread postMessage bridge for ironpad cell execution.
// Exposes the same window.IronpadExecutor API as executor.js, but delegates
// all WASM compilation and execution to a Web Worker via postMessage.
//
// See PRD-0013 for architecture.  The bridge maintains a local isLoaded cache
// so the synchronous `isLoaded(cellId, hash)` call works without a round-trip
// to the Worker.

(function () {
  "use strict";

  // ── BridgeExecutor ────────────────────────────────────────────────────────

  function BridgeExecutor() {
    this._nextId = 1;
    this._pending = new Map();       // reqId -> { resolve, reject }
    this._loadedCache = new Map();   // cellId -> hash
    this._messageHandlers = {};      // type -> handler(msg, cellId)
    this._worker = null;

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
  /// Returns Promise<{ outputBytes, displayText, typeTag }>.
  BridgeExecutor.prototype.execute = function (cellId, inputBytes) {
    return this._postRequest({
      type: "execute",
      cellId: cellId,
      inputBytes: inputBytes,
    });
  };

  /// Remove a loaded cell module.  Fire-and-forget (no response expected).
  BridgeExecutor.prototype.unload = function (cellId) {
    this._loadedCache.delete(cellId);
    this._worker.postMessage({ type: "unload", cellId: cellId });
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

  window.IronpadExecutor = executor;
})();
