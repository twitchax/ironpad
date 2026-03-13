// WASM executor for ironpad cells.
// Provides a global CellExecutor singleton (window.IronpadExecutor) that the
// Rust/WASM side can call via wasm-bindgen to load compiled cell blobs and
// execute them with input/output piping.
//
// Supports two loading modes:
//   1. **wasm-bindgen** (preferred): JS glue module + transformed WASM.
//   2. **raw** (legacy fallback): direct WebAssembly.instantiate.
//
// See MegaPrd §7.5 for architecture and ironpad-cell for the FFI contract.

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

  // ── CellExecutor ───────────────────────────────────────────────────────────

  function CellExecutor() {
    this.modules = new Map(); // cell_id -> { hash, type, ... }
    this._messageHandlers = {}; // type -> handler(msg, cellId)
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

      // 1) Replace bare `import * as __wbg_starN from 'env'` with an
      //    inline shim so the ESM can load from a blob URL.
      var hostShimBody =
        "ironpad_host_message: function(ptr, len) { " +
        "if (window.IronpadExecutor) { " +
        "window.IronpadExecutor._dispatchHostMessage(" +
        JSON.stringify(cellId) + ", ptr, len); } }";
      jsGlue = jsGlue.replace(
        /import\s*\*\s*as\s+(\w+)\s+from\s+['"]env['"]\s*;?/g,
        function (_match, starName) {
          return "var " + starName + " = { " + hostShimBody + " };";
        }
      );

      // 2) Preamble: wrap __wbg_get_imports (fallback for older wasm-bindgen).
      var preamble =
        "var __ironpad_cell_id = " + JSON.stringify(cellId) + ";\n" +
        "if (typeof __wbg_get_imports === 'function') {\n" +
        "  var __ironpad_orig_get_imports = __wbg_get_imports;\n" +
        "  __wbg_get_imports = function() {\n" +
        "    var imports = __ironpad_orig_get_imports();\n" +
        "    if (!imports.env) imports.env = {};\n" +
        "    imports.env.ironpad_host_message = function(ptr, len) {\n" +
        "      if (window.IronpadExecutor) {\n" +
        "        window.IronpadExecutor._dispatchHostMessage(__ironpad_cell_id, ptr, len);\n" +
        "      }\n" +
        "    };\n" +
        "    return imports;\n" +
        "  };\n" +
        "}\n";
      var augmentedGlue = preamble + jsGlue;
      var jsBlob = new Blob([augmentedGlue], { type: "application/javascript" });
      var jsUrl = URL.createObjectURL(jsBlob);

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

  /// Remove a loaded cell module, freeing browser-side resources.
  CellExecutor.prototype.unload = function (cellId) {
    this.modules.delete(cellId);
  };

  /// Check whether a cell has a module loaded with the given hash.
  CellExecutor.prototype.isLoaded = function (cellId, hash) {
    var existing = this.modules.get(cellId);
    return !!existing && existing.hash === hash;
  };

  // ── Expose as a global singleton ─────────────────────────────────────────

  var executor = new CellExecutor();

  // ── Built-in host message handlers ──────────────────────────────────────

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
