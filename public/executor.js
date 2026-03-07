// WASM executor for ironpad cells.
// Provides a global CellExecutor singleton (window.IronpadExecutor) that the
// Rust/WASM side can call via wasm-bindgen to load compiled cell blobs and
// execute them with input/output piping.
//
// See MegaPrd §7.5 for architecture and ironpad-cell for the FFI contract.

(function () {
  "use strict";

  // ── CellResult layout ──────────────────────────────────────────────────────
  //
  // The cell_main function returns a CellResult (#[repr(C)] on wasm32):
  //   offset  0: output_ptr   (u32) — pointer to output bytes
  //   offset  4: output_len   (u32) — length of output bytes
  //   offset  8: display_ptr  (u32) — pointer to UTF-8 display string
  //   offset 12: display_len  (u32) — length of display string
  //
  // Total size: 16 bytes.

  var CELL_RESULT_SIZE = 16;

  // ── CellExecutor ───────────────────────────────────────────────────────────

  function CellExecutor() {
    this.modules = new Map(); // cell_id -> { hash, instance }
  }

  /// Load a compiled WASM blob for a cell.
  ///
  /// If the cell already has a module loaded with the same hash, this is a
  /// no-op (cache hit).  Otherwise the previous module is replaced.
  CellExecutor.prototype.loadBlob = async function (cellId, hash, wasmBytes) {
    var existing = this.modules.get(cellId);
    if (existing && existing.hash === hash) {
      return; // Already loaded, same version.
    }

    // Cell WASM modules are compiled for wasm32-unknown-unknown and are
    // self-contained.  Provide an empty env import namespace to satisfy
    // any linker expectations.
    //
    // Exported by the module:
    //   memory, ironpad_alloc, ironpad_dealloc, cell_main
    var imports = { env: {} };

    var result = await WebAssembly.instantiate(wasmBytes, imports);
    this.modules.set(cellId, { hash: hash, instance: result.instance });
  };

  /// Execute a loaded cell with the given input bytes.
  ///
  /// Returns { outputBytes: Uint8Array, displayText: string | null }.
  ///
  /// Throws on: cell not loaded, missing exports, WASM trap, OOM.
  CellExecutor.prototype.execute = function (cellId, inputBytes) {
    var mod = this.modules.get(cellId);
    if (!mod) {
      throw new Error("Cell " + cellId + " not loaded");
    }

    var instance = mod.instance;
    var memory = instance.exports.memory;
    var alloc = instance.exports.ironpad_alloc;
    var dealloc = instance.exports.ironpad_dealloc;
    var cellMain = instance.exports.cell_main;

    // Validate required exports.
    if (!memory) throw new Error("Cell " + cellId + ": missing 'memory' export");
    if (!alloc) throw new Error("Cell " + cellId + ": missing 'ironpad_alloc' export");
    if (!dealloc) throw new Error("Cell " + cellId + ": missing 'ironpad_dealloc' export");
    if (!cellMain) throw new Error("Cell " + cellId + ": missing 'cell_main' export");

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
    // The Rust-compiled cell_main has signature:
    //   extern "C" fn cell_main(input_ptr: *const u8, input_len: usize) -> CellResult
    //
    // On wasm32, CellResult (16 bytes) exceeds the single-return-value
    // limit, so the compiler uses the "sret" (structural return) convention:
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
    //
    // memory.buffer may have grown during execution, so always re-read it.

    var view = new DataView(memory.buffer);
    var outputPtr = view.getUint32(retptr, true);
    var outputLen = view.getUint32(retptr + 4, true);
    var displayPtr = view.getUint32(retptr + 8, true);
    var displayLen = view.getUint32(retptr + 12, true);

    // Copy output bytes out of WASM memory before freeing.
    var outputBytes = outputLen > 0
      ? new Uint8Array(memory.buffer, outputPtr, outputLen).slice()
      : new Uint8Array(0);

    // Decode display text from UTF-8.
    var displayText = displayLen > 0
      ? new TextDecoder().decode(new Uint8Array(memory.buffer, displayPtr, displayLen))
      : null;

    // ── Clean up all WASM allocations ────────────────────────────────────

    if (inputPtr !== 0) dealloc(inputPtr, inputLen);
    if (outputLen > 0) dealloc(outputPtr, outputLen);
    if (displayLen > 0) dealloc(displayPtr, displayLen);
    if (useSret) dealloc(retptr, CELL_RESULT_SIZE);

    return { outputBytes: outputBytes, displayText: displayText };
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

  window.IronpadExecutor = new CellExecutor();
})();
