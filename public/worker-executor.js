// Core WASM executor for ironpad cells — Worker-safe edition.
// Extracted from executor.js for use inside a Web Worker context.
// Contains zero references to `window`, `document`, or any DOM API.
//
// Supports two loading modes:
//   1. **wasm-bindgen** (preferred): JS glue module + transformed WASM.
//   2. **raw** (legacy fallback): direct WebAssembly.instantiate.
//
// The Worker bootstrap (worker.js) imports this via importScripts() and
// instantiates CellExecutor.

(function () {
  "use strict";

  // ── CellResult layout ──────────────────────────────────────────────────────
  //
  // The cell_main function returns a pointer to a CellResult (#[repr(C)]):
  //   offset  0: output_ptr    (u32) — pointer to output bytes
  //   offset  4: output_len    (u32) — length of output bytes
  //   offset  8: display_ptr   (u32) — pointer to UTF-8 display string
  //   offset 12: display_len   (u32) — length of display string
  //   offset 16: type_tag_ptr  (u32) — pointer to UTF-8 type tag string
  //   offset 20: type_tag_len  (u32) — length of type tag string
  //
  // Total size: 24 bytes.

  var CELL_RESULT_SIZE = 24;

  // ── TickResult layout ──────────────────────────────────────────────────────
  //
  // The cell_tick function returns a pointer to a TickResult (#[repr(C)]):
  //   offset  0: rgb_ptr   (u32) — pointer to RGB pixel bytes
  //   offset  4: rgb_len   (u32) — length of RGB pixel bytes
  //   offset  8: width     (u32) — frame width in pixels
  //   offset 12: height    (u32) — frame height in pixels
  //
  // Total size: 16 bytes.

  var TICK_RESULT_SIZE = 16;

  // ── CellExecutor ───────────────────────────────────────────────────────────

  function CellExecutor() {
    this.modules = new Map(); // cell_id -> { hash, type, ... }
    this._messageHandlers = {}; // type -> handler(msg, cellId)
    this._simBus = new Map(); // key -> { latest: string|null, ring: string[] }
  }

  // ── Host message infrastructure ─────────────────────────────────────────
  //
  // Cells can send JSON messages to the host via `ironpad_host_message`.
  // Messages are dispatched by their `type` field to registered handlers.
  //
  // NOTE: WASM import wiring for `ironpad_host_message` (providing the
  // function in the `env` import namespace so WASM instantiation succeeds)
  // is handled in `loadBlob` for both the raw and wasm-bindgen paths.

  /// Register a handler for a specific host message type.
  CellExecutor.prototype.onHostMessage = function (type, handler) {
    this._messageHandlers[type] = handler;
  };

  /// Read a JSON message from WASM memory and dispatch to the appropriate
  /// handler.  Called by the `ironpad_host_message` import at runtime.
  CellExecutor.prototype._dispatchHostMessage = function (cellId, ptr, len) {
    var entry = this.modules.get(cellId);
    if (!entry) return;

    // Resolve WASM memory from whichever loading path was used.
    var memory = entry.type === "bindgen"
      ? (entry.wasm && entry.wasm.memory)
      : (entry.instance && entry.instance.exports.memory);
    if (!memory) return;

    var bytes = new Uint8Array(memory.buffer, ptr, len);
    var text = new TextDecoder().decode(bytes);

    try {
      var msg = JSON.parse(text);
      var handler = this._messageHandlers[msg.type];
      if (handler) {
        handler(msg, cellId);
      }
    } catch (e) {
      console.warn("ironpad: failed to parse host message:", e);
    }
  };

  /// Read the latest value for a sim bus key from WASM memory.
  /// Allocates WASM memory via ironpad_alloc and writes [u32-LE length][JSON bytes].
  /// Returns the pointer, or 0 if the key has no value.
  CellExecutor.prototype._simRead = function (cellId, keyPtr, keyLen) {
    var entry = this.modules.get(cellId);
    if (!entry) return 0;
    var memory = entry.type === "bindgen"
      ? (entry.wasm && entry.wasm.memory)
      : (entry.instance && entry.instance.exports.memory);
    var alloc = entry.type === "bindgen"
      ? (entry.wasm && entry.wasm.ironpad_alloc)
      : (entry.instance && entry.instance.exports.ironpad_alloc);
    if (!memory || !alloc) return 0;

    var keyBytes = new Uint8Array(memory.buffer, keyPtr, keyLen);
    var key = new TextDecoder().decode(keyBytes);

    var busEntry = this._simBus.get(key);
    if (!busEntry || busEntry.latest === null || busEntry.latest === undefined) return 0;

    var jsonBytes = new TextEncoder().encode(busEntry.latest);
    var totalLen = 4 + jsonBytes.length;
    var ptr = alloc(totalLen);
    if (ptr === 0) return 0;

    var view = new DataView(memory.buffer, ptr, 4);
    view.setUint32(0, jsonBytes.length, true);
    new Uint8Array(memory.buffer, ptr + 4, jsonBytes.length).set(jsonBytes);

    return ptr;
  };

  /// Read all buffered values for a sim bus key from WASM memory.
  /// Writes the ring buffer as a JSON array: [v0,v1,...].
  /// Returns the pointer, or 0 if the key has no entries.
  CellExecutor.prototype._simReadAll = function (cellId, keyPtr, keyLen) {
    var entry = this.modules.get(cellId);
    if (!entry) return 0;
    var memory = entry.type === "bindgen"
      ? (entry.wasm && entry.wasm.memory)
      : (entry.instance && entry.instance.exports.memory);
    var alloc = entry.type === "bindgen"
      ? (entry.wasm && entry.wasm.ironpad_alloc)
      : (entry.instance && entry.instance.exports.ironpad_alloc);
    if (!memory || !alloc) return 0;

    var keyBytes = new Uint8Array(memory.buffer, keyPtr, keyLen);
    var key = new TextDecoder().decode(keyBytes);

    var busEntry = this._simBus.get(key);
    if (!busEntry || busEntry.ring.length === 0) return 0;

    var json = "[" + busEntry.ring.join(",") + "]";
    var jsonBytes = new TextEncoder().encode(json);
    var totalLen = 4 + jsonBytes.length;
    var ptr = alloc(totalLen);
    if (ptr === 0) return 0;

    var view = new DataView(memory.buffer, ptr, 4);
    view.setUint32(0, jsonBytes.length, true);
    new Uint8Array(memory.buffer, ptr + 4, jsonBytes.length).set(jsonBytes);

    return ptr;
  };

  /// Load a compiled WASM blob for a cell.
  ///
  /// If `jsGlue` is provided, uses the wasm-bindgen path: dynamic-imports the
  /// JS glue module and initialises the WASM through it.  Otherwise falls back
  /// to raw `WebAssembly.instantiate`.
  ///
  /// If the cell already has a module loaded with the same hash, this is a
  /// no-op (cache hit).  Otherwise the previous module is replaced.
  CellExecutor.prototype.loadBlob = async function (cellId, hash, wasmBytes, jsGlue) {
    var existing = this.modules.get(cellId);
    if (existing && existing.hash === hash) {
      return; // Already loaded, same version.
    }

    if (jsGlue) {
      // ── wasm-bindgen path ────────────────────────────────────────────
      //
      // The cell's `extern "C" { fn ironpad_host_message(..) }` produces a
      // WASM import under the `env` namespace.  wasm-bindgen (--target web)
      // may emit `import * as __wbg_starN from 'env'` at the top of the
      // ESM glue.  Since we load glue from a blob URL, the browser cannot
      // resolve bare module specifiers — so we rewrite the import into an
      // inline `var` that provides the host-message shim directly.
      //
      // As a belt-and-suspenders fallback (older wasm-bindgen that uses
      // `__wbg_get_imports` without the ESM import), we also prepend a
      // wrapper that injects `env.ironpad_host_message` at import-build
      // time.
      //
      // In Worker context there is no `window`, so the shim captures the
      // executor instance directly via closure.

      var executor = this;
      var escapedCellId = JSON.stringify(cellId);

      // 1) Replace bare `import * as __wbg_starN from 'env'` with an
      //    inline shim so the ESM can load from a blob URL.
      var hostShimBody =
        "ironpad_host_message: function(ptr, len) { " +
        "if (self._ironpadExecutor) { " +
        "self._ironpadExecutor._dispatchHostMessage(" +
        escapedCellId + ", ptr, len); } }, " +
        "ironpad_sim_read: function(ptr, len) { " +
        "return self._ironpadExecutor ? self._ironpadExecutor._simRead(" +
        escapedCellId + ", ptr, len) : 0; }, " +
        "ironpad_sim_read_all: function(ptr, len) { " +
        "return self._ironpadExecutor ? self._ironpadExecutor._simReadAll(" +
        escapedCellId + ", ptr, len) : 0; }";
      jsGlue = jsGlue.replace(
        /import\s*\*\s*as\s+(\w+)\s+from\s+['"]env['"]\s*;?/g,
        function (_match, starName) {
          return "var " + starName + " = { " + hostShimBody + " };";
        }
      );

      // 2) Preamble: wrap __wbg_get_imports (fallback for older wasm-bindgen).
      var preamble =
        "var __ironpad_cell_id = " + escapedCellId + ";\n" +
        "if (typeof __wbg_get_imports === 'function') {\n" +
        "  var __ironpad_orig_get_imports = __wbg_get_imports;\n" +
        "  __wbg_get_imports = function() {\n" +
        "    var imports = __ironpad_orig_get_imports();\n" +
        "    if (!imports.env) imports.env = {};\n" +
        "    imports.env.ironpad_host_message = function(ptr, len) {\n" +
        "      if (self._ironpadExecutor) {\n" +
        "        self._ironpadExecutor._dispatchHostMessage(__ironpad_cell_id, ptr, len);\n" +
        "      }\n" +
        "    };\n" +
        "    imports.env.ironpad_sim_read = function(ptr, len) {\n" +
        "      return self._ironpadExecutor ? self._ironpadExecutor._simRead(__ironpad_cell_id, ptr, len) : 0;\n" +
        "    };\n" +
        "    imports.env.ironpad_sim_read_all = function(ptr, len) {\n" +
        "      return self._ironpadExecutor ? self._ironpadExecutor._simReadAll(__ironpad_cell_id, ptr, len) : 0;\n" +
        "    };\n" +
        "    return imports;\n" +
        "  };\n" +
        "}\n";
      var augmentedGlue = preamble + jsGlue;
      var jsBlob = new Blob([augmentedGlue], { type: "application/javascript" });
      var jsUrl = URL.createObjectURL(jsBlob);

      // Stash the executor on the Worker global so the dynamic-imported
      // glue module (which runs in its own module scope) can reach it.
      self._ironpadExecutor = executor;

      try {
        var mod = await import(/* webpackIgnore: true */ jsUrl);

        // wasm-bindgen's default export is the init function.
        // It returns the raw WASM exports object.
        var wasm = await mod.default({ module_or_path: wasmBytes });

        this.modules.set(cellId, {
          hash: hash,
          type: "bindgen",
          module: mod, // JS glue (wrapped cell_main, handles async)
          wasm: wasm, // Raw WASM exports (memory, ironpad_alloc, ironpad_dealloc)
        });
      } finally {
        URL.revokeObjectURL(jsUrl);
      }
    } else {
      // ── Legacy raw WASM path ─────────────────────────────────────────
      var rawCellId = cellId;
      var rawSelf = this;
      var imports = {
        env: {
          ironpad_host_message: function (ptr, len) {
            rawSelf._dispatchHostMessage(rawCellId, ptr, len);
          },
          ironpad_sim_read: function (ptr, len) {
            return rawSelf._simRead(rawCellId, ptr, len);
          },
          ironpad_sim_read_all: function (ptr, len) {
            return rawSelf._simReadAll(rawCellId, ptr, len);
          },
        },
      };
      var result = await WebAssembly.instantiate(wasmBytes, imports);
      this.modules.set(cellId, {
        hash: hash,
        type: "raw",
        instance: result.instance,
      });
    }
  };

  /// Execute a loaded cell with the given input bytes.
  ///
  /// Returns Promise<{ outputBytes, displayText, typeTag }>.
  ///
  /// Always async: wasm-bindgen cells may have async cell_main (via
  /// wasm-bindgen-futures), and the raw path is wrapped transparently.
  CellExecutor.prototype.execute = async function (cellId, inputBytes) {
    var entry = this.modules.get(cellId);
    if (!entry) {
      throw new Error("Cell " + cellId + " not loaded");
    }

    if (entry.type === "bindgen") {
      return this._executeBindgen(entry, inputBytes);
    } else {
      return this._executeRaw(entry, inputBytes);
    }
  };

  // ── wasm-bindgen execution path ──────────────────────────────────────────
  //
  // Uses the JS glue module's wrapped `cell_main` (which handles async
  // transparently) and the raw WASM exports for memory management.

  CellExecutor.prototype._executeBindgen = async function (entry, inputBytes) {
    var mod = entry.module;
    var wasm = entry.wasm;
    var memory = wasm.memory;
    var alloc = wasm.ironpad_alloc;
    var dealloc = wasm.ironpad_dealloc;

    if (!memory) throw new Error("wasm-bindgen module: missing 'memory' export");
    if (!alloc) throw new Error("wasm-bindgen module: missing 'ironpad_alloc' export");
    if (!dealloc) throw new Error("wasm-bindgen module: missing 'ironpad_dealloc' export");

    // ── Write input bytes into WASM linear memory ────────────────────────

    var inputPtr = 0;
    var inputLen = inputBytes ? inputBytes.length : 0;

    if (inputLen > 0) {
      inputPtr = alloc(inputLen);
      if (inputPtr === 0) {
        throw new Error("ironpad_alloc failed for input (" + inputLen + " bytes)");
      }
      new Uint8Array(memory.buffer, inputPtr, inputLen).set(inputBytes);
    }

    // ── Call cell_main via wasm-bindgen wrapper ──────────────────────────
    //
    // The wrapper handles both sync and async cells: for sync cells it
    // returns a u32 directly; for async cells it returns a Promise<u32>.
    // Awaiting a non-Promise value is a no-op, so this is safe either way.

    var resultPtr;
    try {
      resultPtr = await mod.cell_main(inputPtr, inputLen);
    } catch (e) {
      if (inputPtr !== 0) dealloc(inputPtr, inputLen);
      throw new Error("WASM execution trapped: " + e.message);
    }

    if (!resultPtr) {
      if (inputPtr !== 0) dealloc(inputPtr, inputLen);
      throw new Error("cell_main returned null");
    }

    // ── Read CellResult from WASM memory ─────────────────────────────────

    return this._readCellResult(memory, alloc, dealloc, resultPtr, inputPtr, inputLen, false);
  };

  // ── Legacy raw WASM execution path ───────────────────────────────────────
  //
  // Direct WebAssembly instance access with sret calling convention detection.

  CellExecutor.prototype._executeRaw = function (entry, inputBytes) {
    var instance = entry.instance;
    var memory = instance.exports.memory;
    var alloc = instance.exports.ironpad_alloc;
    var dealloc = instance.exports.ironpad_dealloc;
    var cellMain = instance.exports.cell_main;

    // Validate required exports.
    if (!memory) throw new Error("raw module: missing 'memory' export");
    if (!alloc) throw new Error("raw module: missing 'ironpad_alloc' export");
    if (!dealloc) throw new Error("raw module: missing 'ironpad_dealloc' export");
    if (!cellMain) throw new Error("raw module: missing 'cell_main' export");

    // ── Write input bytes into WASM linear memory ────────────────────────

    var inputPtr = 0;
    var inputLen = inputBytes ? inputBytes.length : 0;

    if (inputLen > 0) {
      inputPtr = alloc(inputLen);
      if (inputPtr === 0) {
        throw new Error("ironpad_alloc failed for input (" + inputLen + " bytes)");
      }
      new Uint8Array(memory.buffer, inputPtr, inputLen).set(inputBytes);
    }

    // ── Call cell_main ───────────────────────────────────────────────────
    //
    // On wasm32, CellResult (24 bytes) exceeds the single-return-value
    // limit, so the compiler may use the "sret" (structural return)
    // convention:
    //   cell_main(retptr: i32, input_ptr: i32, input_len: i32) -> void
    //
    // We detect the convention by inspecting the exported function's arity:
    //   3 parameters → sret convention (retptr + input_ptr + input_len)
    //   2 parameters → direct pointer return (returns *const CellResult)

    var retptr;
    var useSret = cellMain.length === 3;

    if (useSret) {
      retptr = alloc(CELL_RESULT_SIZE);
      if (retptr === 0) {
        if (inputPtr !== 0) dealloc(inputPtr, inputLen);
        throw new Error("ironpad_alloc failed for return struct");
      }
    }

    try {
      if (useSret) {
        cellMain(retptr, inputPtr, inputLen);
      } else {
        retptr = cellMain(inputPtr, inputLen);
        if (!retptr) {
          throw new Error("cell_main returned null");
        }
      }
    } catch (e) {
      // Clean up on WASM trap.
      if (inputPtr !== 0) dealloc(inputPtr, inputLen);
      if (useSret && retptr) dealloc(retptr, CELL_RESULT_SIZE);
      throw new Error("WASM execution trapped: " + e.message);
    }

    // ── Read CellResult from WASM memory ─────────────────────────────────

    return this._readCellResult(memory, alloc, dealloc, retptr, inputPtr, inputLen, useSret);
  };

  // ── Shared CellResult reader ─────────────────────────────────────────────
  //
  // Reads the 24-byte CellResult struct, copies data out, and frees all WASM
  // allocations.  memory.buffer may have grown during execution, so it is
  // always re-read here.

  CellExecutor.prototype._readCellResult = function (
    memory, alloc, dealloc, retptr, inputPtr, inputLen, useSret
  ) {
    var view = new DataView(memory.buffer);
    var outputPtr = view.getUint32(retptr, true);
    var outputLen = view.getUint32(retptr + 4, true);
    var displayPtr = view.getUint32(retptr + 8, true);
    var displayLen = view.getUint32(retptr + 12, true);
    var typeTagPtr = view.getUint32(retptr + 16, true);
    var typeTagLen = view.getUint32(retptr + 20, true);

    // Copy output bytes out of WASM memory before freeing.
    var outputBytes = outputLen > 0
      ? new Uint8Array(memory.buffer, outputPtr, outputLen).slice()
      : new Uint8Array(0);

    // Decode display text from UTF-8.
    var displayText = displayLen > 0
      ? new TextDecoder().decode(new Uint8Array(memory.buffer, displayPtr, displayLen))
      : null;

    // Decode type tag from UTF-8.
    var typeTag = typeTagLen > 0
      ? new TextDecoder().decode(new Uint8Array(memory.buffer, typeTagPtr, typeTagLen))
      : null;

    // ── Clean up all WASM allocations ────────────────────────────────────

    if (inputPtr !== 0) dealloc(inputPtr, inputLen);
    if (outputLen > 0) dealloc(outputPtr, outputLen);
    if (displayLen > 0) dealloc(displayPtr, displayLen);
    if (typeTagLen > 0) dealloc(typeTagPtr, typeTagLen);
    // For sret, we allocated retptr ourselves; for bindgen, the cell leaked
    // a Box<CellResult> that we must free.
    if (useSret || retptr) dealloc(retptr, CELL_RESULT_SIZE);

    return { outputBytes: outputBytes, displayText: displayText, typeTag: typeTag };
  };

  // ── Tick execution ───────────────────────────────────────────────────────
  //
  // Simulation cells export `cell_tick()` which advances one simulation step
  // and returns a TickResult (16 bytes) containing the RGB frame data.
  // The WASM module stays loaded between ticks — state persists in a static.

  /// Execute a single tick on a loaded simulation cell.
  ///
  /// Returns Promise<{ width, height, rgbBytes }>.
  CellExecutor.prototype.tick = async function (cellId) {
    var entry = this.modules.get(cellId);
    if (!entry) {
      throw new Error("Cell " + cellId + " not loaded");
    }

    if (entry.type === "bindgen") {
      return this._tickBindgen(entry);
    } else {
      return this._tickRaw(entry);
    }
  };

  // ── wasm-bindgen tick path ──────────────────────────────────────────────

  CellExecutor.prototype._tickBindgen = async function (entry) {
    var mod = entry.module;
    var wasm = entry.wasm;
    var memory = wasm.memory;
    var dealloc = wasm.ironpad_dealloc;

    if (!memory) throw new Error("wasm-bindgen module: missing 'memory' export");
    if (!dealloc) throw new Error("wasm-bindgen module: missing 'ironpad_dealloc' export");

    // cell_tick may be wrapped by wasm-bindgen (on mod) or a raw export (on wasm).
    var tickFn = mod.cell_tick || wasm.cell_tick;
    if (!tickFn) throw new Error("Module does not export cell_tick");

    var resultPtr;
    try {
      resultPtr = await tickFn();
    } catch (e) {
      throw new Error("WASM tick trapped: " + e.message);
    }

    if (!resultPtr) {
      throw new Error("cell_tick returned null");
    }

    return this._readTickResult(memory, dealloc, resultPtr, false);
  };

  // ── Legacy raw tick path ────────────────────────────────────────────────

  CellExecutor.prototype._tickRaw = function (entry) {
    var instance = entry.instance;
    var memory = instance.exports.memory;
    var alloc = instance.exports.ironpad_alloc;
    var dealloc = instance.exports.ironpad_dealloc;
    var cellTick = instance.exports.cell_tick;

    if (!memory) throw new Error("raw module: missing 'memory' export");
    if (!dealloc) throw new Error("raw module: missing 'ironpad_dealloc' export");
    if (!cellTick) throw new Error("raw module: missing 'cell_tick' export");

    // sret detection: cell_tick has 0 params (direct return) or 1 param (sret).
    var retptr;
    var useSret = cellTick.length === 1;

    if (useSret) {
      if (!alloc) throw new Error("raw module: missing 'ironpad_alloc' export");
      retptr = alloc(TICK_RESULT_SIZE);
      if (retptr === 0) {
        throw new Error("ironpad_alloc failed for tick return struct");
      }
    }

    try {
      if (useSret) {
        cellTick(retptr);
      } else {
        retptr = cellTick();
        if (!retptr) {
          throw new Error("cell_tick returned null");
        }
      }
    } catch (e) {
      if (useSret && retptr) dealloc(retptr, TICK_RESULT_SIZE);
      throw new Error("WASM tick trapped: " + e.message);
    }

    return this._readTickResult(memory, dealloc, retptr, useSret);
  };

  // ── Shared TickResult reader ────────────────────────────────────────────

  CellExecutor.prototype._readTickResult = function (
    memory, dealloc, retptr, useSret
  ) {
    var view = new DataView(memory.buffer);
    var rgbPtr = view.getUint32(retptr, true);
    var rgbLen = view.getUint32(retptr + 4, true);
    var width = view.getUint32(retptr + 8, true);
    var height = view.getUint32(retptr + 12, true);

    // Copy RGB bytes out of WASM memory before freeing.
    var rgbBytes = rgbLen > 0
      ? new Uint8Array(memory.buffer, rgbPtr, rgbLen).slice()
      : new Uint8Array(0);

    // ── Clean up WASM allocations ────────────────────────────────────────

    if (rgbLen > 0) dealloc(rgbPtr, rgbLen);
    if (useSret || retptr) dealloc(retptr, TICK_RESULT_SIZE);

    return { width: width, height: height, rgbBytes: rgbBytes };
  };

  /// Remove a loaded cell module, freeing browser-side resources.
  CellExecutor.prototype.unload = function (cellId) {
    this.modules.delete(cellId);
  };

  /// Check whether a cell has a module loaded with the given hash.
  CellExecutor.prototype.isLoaded = function (cellId, hash) {
    var existing = this.modules.get(cellId);
    return !!existing && existing.hash === hash;
  };

  /// Write a value to the sim bus directly (e.g. forwarded from main thread via sim_bus_write).
  CellExecutor.prototype.simBusWrite = function (key, value) {
    var json = JSON.stringify(value);
    var entry = this._simBus.get(key);
    if (!entry) {
      entry = { latest: null, ring: [] };
      this._simBus.set(key, entry);
    }
    entry.latest = json;
    entry.ring.push(json);
    if (entry.ring.length > 1000) entry.ring.shift();
  };

  // ── Expose on Worker global ─────────────────────────────────────────────

  self.CellExecutor = CellExecutor;
})();
