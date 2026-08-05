// Shared CellExecutor core for ironpad cells.
// Contains all constants, helpers, GPU state, and CellExecutor prototype methods.
// Loaded by both executor.js (main-thread) and executor-worker-core.js (Worker).
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

  // ── LiveTickResult layout ────────────────────────────────────────────────
  //
  // The cell_tick function for LiveView cells returns a pointer to a
  // LiveTickResult (#[repr(C)]):
  //   offset  0: kind         (u32) — 0=Text, 1=Html, 2=Markdown
  //   offset  4: content_ptr  (u32) — pointer to UTF-8 content string
  //   offset  8: content_len  (u32) — length of content string
  //
  // Total size: 12 bytes.

  var LIVE_TICK_RESULT_SIZE = 12;

  // ── Sim bus ────────────────────────────────────────────────────────────────
  //
  // Max entries retained per sim-bus key's ring buffer. executor-bridge.js keeps
  // a standalone copy of this cap (it can't reuse this module — see updateSimBus).

  var SIM_BUS_RING_CAP = 1000;

  // ── JSPI (blocking host calls, PRD-0043) ──────────────────────────────────
  //
  // Cells that import `ironpad_blocking_*` (ironpad_cell::blocking) need the
  // WebAssembly JS Promise Integration API: their imports are wrapped in
  // WebAssembly.Suspending and cell_main is entered via WebAssembly.promising,
  // so a synchronous Rust call can suspend the whole WASM stack while a JS
  // promise settles. Shipped by default in Chrome/Edge 137+.

  var JSPI_IMPORT_PREFIX = "ironpad_blocking_";
  var JSPI_UNSUPPORTED_MSG =
    "This cell uses blocking host calls (ironpad_cell::blocking), which need " +
    "WebAssembly JSPI: available in Chrome and Edge 137+, not yet in Firefox " +
    "or Safari.";

  function _jspiAvailable() {
    return (
      typeof WebAssembly.Suspending === "function" &&
      typeof WebAssembly.promising === "function"
    );
  }

  /// True when the compiled module imports any `ironpad_blocking_*` function,
  /// i.e. it must be entered via WebAssembly.promising in execute().
  function _moduleNeedsJspi(module) {
    return WebAssembly.Module.imports(module).some(function (imp) {
      return imp.module === "env" && imp.name.indexOf(JSPI_IMPORT_PREFIX) === 0;
    });
  }

  // ── WASM trap diagnostics ─────────────────────────────────────────────────
  //
  // When a WASM module traps, the JS engine throws a WebAssembly.RuntimeError
  // with a terse message like "unreachable".  This helper inspects the error
  // and the module's memory state to produce a more actionable description.

  function _describeWasmTrap(e, memory) {
    var raw = (e && e.message) ? e.message : String(e);

    // Report current linear-memory size for context.
    var memHint = "";
    if (memory) {
      var bytes = memory.buffer.byteLength;
      var mb = (bytes / (1024 * 1024)).toFixed(0);
      memHint = " (linear memory: " + mb + " MB)";
    }

    if (/out of bounds memory access/i.test(raw)) {
      return "Out-of-bounds memory access" + memHint +
        " — the cell tried to read or write outside allocated memory.";
    }

    if (/call stack exhausted/i.test(raw) || /stack overflow/i.test(raw) ||
        /Maximum call stack/i.test(raw)) {
      return "Stack overflow — the cell likely has unbounded or very deep recursion.";
    }

    if (/unreachable/i.test(raw)) {
      return "Execution trapped" + memHint +
        " — this usually means the cell ran out of memory. " +
        "Try reducing the problem size (e.g., lower the resolution or iteration count).";
    }

    // Fall back to the raw message with memory context.
    return raw + memHint;
  }

  // ── GPU state ──────────────────────────────────────────────────────────────
  //
  // WebGPU capability detection, device initialization, handle registry, and
  // FFI helpers.  All GPU handles live in a flat Map so WASM cells can
  // reference them by integer handle.

  var _gpuDevice = null;
  var _gpuAvailable = null; // null = not yet probed, true/false = result
  var _gpuHandles = new Map();
  var _gpuNextHandle = 1;

  async function _initGpu() {
    if (_gpuAvailable !== null) return _gpuAvailable;
    try {
      if (typeof navigator === "undefined" || !navigator.gpu) {
        _gpuAvailable = false;
        return false;
      }
      var adapter = await navigator.gpu.requestAdapter();
      if (!adapter) {
        _gpuAvailable = false;
        return false;
      }
      _gpuDevice = await adapter.requestDevice({
        requiredLimits: {
          maxStorageBufferBindingSize: adapter.limits.maxStorageBufferBindingSize,
          maxBufferSize: adapter.limits.maxBufferSize,
        },
      });
      _gpuDevice.lost.then(function (info) {
        console.warn("ironpad: GPU device lost:", info.message);
        _gpuDevice = null;
        _gpuAvailable = null;
      });
      _gpuAvailable = true;
      return true;
    } catch (e) {
      console.warn("ironpad: GPU init failed:", e);
      _gpuAvailable = false;
      return false;
    }
  }

  function _gpuCleanupHandles(handles) {
    for (var h of handles) {
      var res = _gpuHandles.get(h);
      if (res) {
        if (res.destroy) res.destroy();
        _gpuHandles.delete(h);
      }
    }
  }

  // ── GPU FFI helpers ────────────────────────────────────────────────────────

  function _gpuAvailableSync() {
    return _gpuAvailable === true ? 1 : 0;
  }

  function _gpuCreateBuffer(size, usage) {
    // usage: 0=STORAGE, 1=STORAGE|COPY_SRC, 2=MAP_READ|COPY_DST, 3=STORAGE|COPY_DST
    if (!_gpuDevice) return 0;
    var gpuUsage;
    switch (usage) {
      case 0: gpuUsage = GPUBufferUsage.STORAGE; break;
      case 1: gpuUsage = GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_SRC; break;
      case 2: gpuUsage = GPUBufferUsage.MAP_READ | GPUBufferUsage.COPY_DST; break;
      case 3: gpuUsage = GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST; break;
      default: gpuUsage = GPUBufferUsage.STORAGE; break;
    }
    try {
      var buf = _gpuDevice.createBuffer({ size: size, usage: gpuUsage });
      var handle = _gpuNextHandle++;
      _gpuHandles.set(handle, buf);
      return handle;
    } catch (e) {
      console.error("ironpad: GPU createBuffer failed:", e);
      return 0;
    }
  }

  function _gpuWriteBuffer(handle, srcPtr, srcLen, memory) {
    if (!_gpuDevice) return;
    var buf = _gpuHandles.get(handle);
    if (!buf) return;
    var data = new Uint8Array(memory.buffer, srcPtr, srcLen);
    _gpuDevice.queue.writeBuffer(buf, 0, data);
  }

  function _gpuDispatchComputeSync(
    shaderPtr, shaderLen, uniformHandle, outputHandle, width, height, memory
  ) {
    if (!_gpuDevice) return 1;
    try {
      var shaderBytes = new Uint8Array(memory.buffer, shaderPtr, shaderLen);
      var shaderCode = new TextDecoder().decode(shaderBytes);

      var shaderModule = _gpuDevice.createShaderModule({ code: shaderCode });

      var uniformBuf = _gpuHandles.get(uniformHandle);
      var outputBuf = _gpuHandles.get(outputHandle);
      if (!uniformBuf || !outputBuf) return 1;

      var bindGroupLayout = _gpuDevice.createBindGroupLayout({
        entries: [
          { binding: 0, visibility: GPUShaderStage.COMPUTE, buffer: { type: "read-only-storage" } },
          { binding: 1, visibility: GPUShaderStage.COMPUTE, buffer: { type: "storage" } },
        ],
      });

      var bindGroup = _gpuDevice.createBindGroup({
        layout: bindGroupLayout,
        entries: [
          { binding: 0, resource: { buffer: uniformBuf } },
          { binding: 1, resource: { buffer: outputBuf } },
        ],
      });

      var pipeline = _gpuDevice.createComputePipeline({
        layout: _gpuDevice.createPipelineLayout({ bindGroupLayouts: [bindGroupLayout] }),
        compute: { module: shaderModule, entryPoint: "main" },
      });

      var commandEncoder = _gpuDevice.createCommandEncoder();
      var pass = commandEncoder.beginComputePass();
      pass.setPipeline(pipeline);
      pass.setBindGroup(0, bindGroup);
      pass.dispatchWorkgroups(Math.ceil(width / 16), Math.ceil(height / 16));
      pass.end();
      _gpuDevice.queue.submit([commandEncoder.finish()]);
      return 0;
    } catch (e) {
      console.error("ironpad: GPU dispatch failed:", e);
      return 1;
    }
  }

  // ── GPU async readback (called after cell_main returns) ────────────────────

  async function _gpuReadPixels(outputHandle, stagingHandle, width, height) {
    if (!_gpuDevice) return null;
    var outputBuf = _gpuHandles.get(outputHandle);
    var stagingBuf = _gpuHandles.get(stagingHandle);
    if (!outputBuf || !stagingBuf) return null;
    try {
      var byteSize = width * height * 16; // vec4<f32> = 16 bytes per pixel
      var commandEncoder = _gpuDevice.createCommandEncoder();
      commandEncoder.copyBufferToBuffer(outputBuf, 0, stagingBuf, 0, byteSize);
      _gpuDevice.queue.submit([commandEncoder.finish()]);

      await stagingBuf.mapAsync(GPUMapMode.READ);
      var mapped = new Float32Array(stagingBuf.getMappedRange());

      var pixelCount = width * height;
      var rgb = new Uint8Array(pixelCount * 3);
      for (var i = 0; i < pixelCount; i++) {
        rgb[i * 3] = Math.min(255, Math.max(0, Math.round(mapped[i * 4] * 255)));
        rgb[i * 3 + 1] = Math.min(255, Math.max(0, Math.round(mapped[i * 4 + 1] * 255)));
        rgb[i * 3 + 2] = Math.min(255, Math.max(0, Math.round(mapped[i * 4 + 2] * 255)));
      }
      stagingBuf.unmap();
      return rgb;
    } catch (e) {
      console.error("ironpad: GPU readPixels failed:", e);
      return null;
    }
  }

  // ── BMP construction (used for GPU canvas output) ──────────────────────────

  function _gpuBuildBmp(width, height, rgb) {
    var rowStride = Math.ceil((width * 3) / 4) * 4;
    var pixelDataSize = rowStride * height;
    var fileSize = 54 + pixelDataSize;
    var bmp = new Uint8Array(fileSize);
    var view = new DataView(bmp.buffer);

    // BMP file header (14 bytes).
    bmp[0] = 0x42; bmp[1] = 0x4d; // "BM"
    view.setUint32(2, fileSize, true);
    view.setUint32(10, 54, true);

    // DIB header (BITMAPINFOHEADER, 40 bytes).
    view.setUint32(14, 40, true);
    view.setInt32(18, width, true);
    view.setInt32(22, -height, true); // negative = top-down row order
    view.setUint16(26, 1, true);
    view.setUint16(28, 24, true);
    view.setUint32(34, pixelDataSize, true);

    for (var y = 0; y < height; y++) {
      for (var x = 0; x < width; x++) {
        var srcIdx = (y * width + x) * 3;
        var dstIdx = 54 + y * rowStride + x * 3;
        bmp[dstIdx] = rgb[srcIdx + 2];     // B
        bmp[dstIdx + 1] = rgb[srcIdx + 1]; // G
        bmp[dstIdx + 2] = rgb[srcIdx];     // R
      }
    }
    return bmp;
  }

  function _gpuBmpToBase64DataUrl(bmp) {
    var binary = "";
    for (var i = 0; i < bmp.length; i++) {
      binary += String.fromCharCode(bmp[i]);
    }
    return "data:image/bmp;base64," + btoa(binary);
  }

  // ── CellExecutor ───────────────────────────────────────────────────────────

  function CellExecutor(globalRef) {
    this._globalRef = globalRef;
    this.modules = new Map(); // cell_id -> { hash, type, ... }
    this._messageHandlers = {}; // type -> handler(msg, cellId)
    this._simBus = new Map(); // key -> { latest: string|null, ring: string[] }
    this._blockingPayloads = new Map(); // cell_id -> { ok: bool, bytes: Uint8Array }
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

    var bytes = new Uint8Array(memory.buffer, ptr, len).slice();
    var text = new TextDecoder().decode(bytes);

    try {
      var msg = JSON.parse(text);

      // GPU readback requests are deferred until after cell_main returns.
      if (msg.type === "gpu_read_pixels") {
        if (!this._pendingGpuReadbacks) this._pendingGpuReadbacks = [];
        this._pendingGpuReadbacks.push(msg);
        return;
      }

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

    var keyBytes = new Uint8Array(memory.buffer, keyPtr, keyLen).slice();
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

    var keyBytes = new Uint8Array(memory.buffer, keyPtr, keyLen).slice();
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

  // ── GPU executor methods (resolve WASM memory per cell) ─────────────────

  CellExecutor.prototype._gpuAvailableSync = function () {
    return _gpuAvailableSync();
  };

  CellExecutor.prototype._gpuCreateBuffer = function (size, usage) {
    return _gpuCreateBuffer(size, usage);
  };

  CellExecutor.prototype._gpuWriteBufferForCell = function (cellId, handle, ptr, len) {
    var entry = this.modules.get(cellId);
    if (!entry) return;
    var memory = entry.type === "bindgen"
      ? (entry.wasm && entry.wasm.memory)
      : (entry.instance && entry.instance.exports.memory);
    if (memory) _gpuWriteBuffer(handle, ptr, len, memory);
  };

  CellExecutor.prototype._gpuDispatchComputeForCell = function (
    cellId, shaderPtr, shaderLen, uniformHandle, outputHandle, width, height
  ) {
    var entry = this.modules.get(cellId);
    if (!entry) return 1;
    var memory = entry.type === "bindgen"
      ? (entry.wasm && entry.wasm.memory)
      : (entry.instance && entry.instance.exports.memory);
    if (!memory) return 1;
    return _gpuDispatchComputeSync(
      shaderPtr, shaderLen, uniformHandle, outputHandle, width, height, memory
    );
  };

  /// Process any deferred GPU readback requests after cell_main returns.
  /// Replaces placeholder BlobImage panels in the structured JSON displayText
  /// with the actual GPU-rendered image data.
  CellExecutor.prototype._processGpuReadbacks = async function (cellResult) {
    if (!this._pendingGpuReadbacks || this._pendingGpuReadbacks.length === 0) {
      return cellResult;
    }

    // Parse the structured panels JSON so we can replace BlobImage panels.
    var panels = null;
    if (cellResult.displayText) {
      try { panels = JSON.parse(cellResult.displayText); } catch (e) { panels = null; }
    }

    for (var rb of this._pendingGpuReadbacks) {
      var rgb = await _gpuReadPixels(
        rb.output_handle, rb.staging_handle, rb.width, rb.height
      );
      if (rgb) {
        var bmp = _gpuBuildBmp(rb.width, rb.height, rgb);
        // Extract raw base64 (no data URL prefix) for the BlobImage panel.
        var binary = "";
        for (var i = 0; i < bmp.length; i++) {
          binary += String.fromCharCode(bmp[i]);
        }
        var base64 = btoa(binary);

        if (panels && Array.isArray(panels)) {
          // Find the first BlobImage panel matching this readback's dimensions
          // and replace its placeholder base64_data with the real GPU output.
          var found = false;
          for (var j = 0; j < panels.length; j++) {
            var p = panels[j];
            if (p && p.BlobImage &&
                p.BlobImage.width === rb.width &&
                p.BlobImage.height === rb.height) {
              p.BlobImage.base64_data = base64;
              found = true;
              break;
            }
          }
          if (!found) {
            // No matching placeholder — append a new BlobImage panel.
            panels.push({
              BlobImage: {
                mime_type: "image/bmp",
                base64_data: base64,
                width: rb.width,
                height: rb.height
              }
            });
          }
        } else {
          // No valid panels array — create one with the GPU result.
          panels = [{
            BlobImage: {
              mime_type: "image/bmp",
              base64_data: base64,
              width: rb.width,
              height: rb.height
            }
          }];
        }
      }
    }

    if (panels) {
      cellResult.displayText = JSON.stringify(panels);
    }
    this._pendingGpuReadbacks = [];
    return cellResult;
  };

  /// Load a compiled WASM blob for a cell.
  ///
  /// If `jsGlue` is provided, uses the wasm-bindgen path: dynamic-imports the
  /// JS glue module and initialises the WASM through it.  Otherwise falls back
  /// to raw `WebAssembly.instantiate`.
  ///
  /// If the cell already has a module loaded with the same hash, this is a
  /// no-op (cache hit).  Otherwise the previous module is replaced.
  // ── JSPI blocking-call host side (PRD-0043) ──────────────────────────────
  //
  // Two-phase fetch protocol, mirroring `_simRead`'s host->cell copy shape:
  // the suspending `ironpad_blocking_fetch` performs the request while the
  // instance is parked and stashes the payload per-cell, returning only its
  // length; after the cell resumes, the synchronous `ironpad_blocking_read`
  // copies the payload into a buffer the cell allocated itself. The host never
  // calls back into a suspended instance.

  /// Wrap `fn` as a suspending import when JSPI is available; otherwise a stub
  /// that traps with a clear browser-support message. Instantiation succeeds
  /// either way — execute() gates earlier with the same message, so the stub
  /// is a belt-and-suspenders backstop.
  CellExecutor.prototype._jspiImport = function (fn) {
    if (_jspiAvailable()) return new WebAssembly.Suspending(fn);
    return function () {
      throw new Error(JSPI_UNSUPPORTED_MSG);
    };
  };

  /// Linear memory of a loaded cell (either loading path), or null.
  CellExecutor.prototype._cellMemory = function (cellId) {
    var entry = this.modules.get(cellId);
    if (!entry) return null;
    return entry.type === "bindgen"
      ? entry.wasm.memory
      : entry.instance.exports.memory;
  };

  /// Suspending import: fetch the URL (read from WASM memory), stash the
  /// payload (body on success, error text otherwise), return its byte length.
  CellExecutor.prototype._blockingFetch = async function (cellId, urlPtr, urlLen) {
    var memory = this._cellMemory(cellId);
    // Copy the URL out before the first await: the view must not be held
    // across a suspension (a memory.grow on resume would detach it).
    var url = "";
    if (memory) {
      var urlBytes = new Uint8Array(urlLen);
      urlBytes.set(new Uint8Array(memory.buffer, urlPtr, urlLen));
      url = new TextDecoder().decode(urlBytes);
    }
    var ok = false;
    var bytes;
    try {
      var resp = await fetch(url);
      var body = new Uint8Array(await resp.arrayBuffer());
      if (resp.ok) {
        ok = true;
        bytes = body;
      } else {
        bytes = new TextEncoder().encode(
          "HTTP " + resp.status + (resp.statusText ? " " + resp.statusText : "")
        );
      }
    } catch (e) {
      bytes = new TextEncoder().encode(String((e && e.message) || e));
    }
    this._blockingPayloads.set(cellId, { ok: ok, bytes: bytes });
    return bytes.length;
  };

  /// Synchronous import: 1 if the last fetch for this cell succeeded.
  CellExecutor.prototype._blockingFetchOk = function (cellId) {
    var p = this._blockingPayloads.get(cellId);
    return p && p.ok ? 1 : 0;
  };

  /// Synchronous import: copy the stashed payload into a cell-allocated
  /// buffer, returning the byte count copied.
  CellExecutor.prototype._blockingRead = function (cellId, bufPtr, bufLen) {
    var memory = this._cellMemory(cellId);
    var p = this._blockingPayloads.get(cellId);
    if (!memory || !p) return 0;
    var n = Math.min(bufLen, p.bytes.length);
    new Uint8Array(memory.buffer, bufPtr, n).set(p.bytes.subarray(0, n));
    return n;
  };

  // ── env host-import table ─────────────────────────────────────────────
  //
  // THE single source of truth for the `env` imports a cell's WASM module
  // may declare. The authoritative Rust side is the `#[link(wasm_import_module
  // = "env")] extern "C"` blocks in ironpad-cell (lib.rs, sim.rs, gpu.rs,
  // blocking.rs); this table must list exactly those names.
  //
  // Each entry builds the import's JS function EXPRESSION as a string,
  // parameterized by:
  //   - g:   an expression evaluating to the executor object;
  //   - cid: an expression evaluating to the cell id.
  // Three consumers generate from it: the ESM `import * from 'env'` rewrite,
  // the `__wbg_get_imports` preamble (older wasm-bindgen fallback), and the
  // raw-WASM imports object. They used to be three hand-written copies, and
  // a new import missing from ONE copy failed instantiation only on that
  // specific loading path — which no test exercises.
  function _envImportExprs(g, cid) {
    // The Suspending wrappers are built inline (never via the executor
    // global): the generated text also evaluates inside rayon sub-workers,
    // where the executor global is undefined — a build-time dereference
    // would crash worker init before the thread pool exists.
    function jspiGuarded(fnExpr) {
      return (
        "(typeof WebAssembly.Suspending === 'function') " +
        "? new WebAssembly.Suspending(" + fnExpr + ") " +
        ": function() { throw new Error(" + JSON.stringify(JSPI_UNSUPPORTED_MSG) + "); }"
      );
    }
    return {
      ironpad_host_message:
        "function(ptr, len) { if (" + g + ") { " + g + "._dispatchHostMessage(" + cid + ", ptr, len); } }",
      ironpad_sim_read:
        "function(ptr, len) { return " + g + " ? " + g + "._simRead(" + cid + ", ptr, len) : 0; }",
      ironpad_sim_read_all:
        "function(ptr, len) { return " + g + " ? " + g + "._simReadAll(" + cid + ", ptr, len) : 0; }",
      ironpad_gpu_available:
        "function() { return " + g + "._gpuAvailableSync(); }",
      ironpad_gpu_create_buffer:
        "function(s, u) { return " + g + "._gpuCreateBuffer(s, u); }",
      ironpad_gpu_write_buffer:
        "function(h, p, l) { if (" + g + ") " + g + "._gpuWriteBufferForCell(" + cid + ", h, p, l); }",
      ironpad_gpu_dispatch_compute:
        "function(sp, sl, uh, oh, w, h) { return " + g + " ? " + g + "._gpuDispatchComputeForCell(" + cid + ", sp, sl, uh, oh, w, h) : 1; }",
      ironpad_blocking_sleep_ms: jspiGuarded(
        "function(ms) { return new Promise(function(res) { setTimeout(res, ms); }); }"
      ),
      ironpad_blocking_fetch: jspiGuarded(
        "function(p, l) { return " + g + "._blockingFetch(" + cid + ", p, l); }"
      ),
      ironpad_blocking_fetch_ok:
        "function() { return " + g + " ? " + g + "._blockingFetchOk(" + cid + ") : 0; }",
      ironpad_blocking_read:
        "function(p, l) { return " + g + " ? " + g + "._blockingRead(" + cid + ", p, l) : 0; }",
    };
  }

  CellExecutor.prototype.loadBlob = async function (cellId, hash, wasmBytes, jsGlue) {
    var existing = this.modules.get(cellId);
    if (existing && existing.hash === hash) {
      return; // Already loaded, same version.
    }

    var globalRef = this._globalRef;

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

      var escapedCellId = JSON.stringify(cellId);

      // 1) Replace bare `import * as __wbg_starN from 'env'` with an
      //    inline shim so the ESM can load from a blob URL.
      // Generated from the single env-import table above.
      var _shimExprs = _envImportExprs(globalRef, escapedCellId);
      var hostShimBody = Object.keys(_shimExprs)
        .map(function (k) { return k + ": " + _shimExprs[k]; })
        .join(", ");

      jsGlue = jsGlue.replace(
        /import\s*\*\s*as\s+(\w+)\s+from\s+['"]env['"]\s*;?/g,
        function (_match, starName) {
          return "var " + starName + " = { " + hostShimBody + " };";
        }
      );

      // 1b) Replace wasm-bindgen-rayon snippet imports with an inline
      //     startWorkers implementation.  The snippet lives at a relative
      //     path like `./snippets/wasm-bindgen-rayon-HASH/src/workerHelpers.js`
      //     which cannot be resolved when the glue is loaded from a blob URL.
      //
      //     Our inline version creates rayon sub-workers from blob URLs,
      //     passing the rewritten glue code so each sub-worker can import
      //     it and call `pkg.default(init)` + `pkg.wbg_rayon_start_worker`.
      jsGlue = jsGlue.replace(
        /import\s*\{\s*startWorkers\s*\}\s+from\s+['"][^'"]*wasm-bindgen-rayon[^'"]*workerHelpers\.js['"];?/g,
        "\n" +
        "var _rayonWorkers;\n" +
        "var _RAYON_WORKER_INIT_TIMEOUT_MS = 30000;\n" +
        "// Resolve when `target` posts a message of `type`; reject on worker\n" +
        "// error or after `timeoutMs`. Both listeners AND the timer are always\n" +
        "// torn down so a sub-worker that never signals ready can't leak the\n" +
        "// listener or hang initThreadPool forever.\n" +
        "function _waitForMsgType(target, type, timeoutMs) {\n" +
        "  return new Promise(function(resolve, reject) {\n" +
        "    function cleanup() {\n" +
        "      clearTimeout(timer);\n" +
        "      target.removeEventListener('message', onMsg);\n" +
        "      target.removeEventListener('error', onErr);\n" +
        "    }\n" +
        "    var timer = setTimeout(function() {\n" +
        "      cleanup();\n" +
        "      reject(new Error('rayon worker init timed out after ' + timeoutMs + 'ms'));\n" +
        "    }, timeoutMs);\n" +
        "    function onMsg(event) {\n" +
        "      if (!event.data || event.data.type !== type) return;\n" +
        "      cleanup();\n" +
        "      resolve(event.data);\n" +
        "    }\n" +
        "    function onErr(event) {\n" +
        "      cleanup();\n" +
        "      reject(new Error('rayon worker failed to initialize: ' + (event.message || 'unknown error')));\n" +
        "    }\n" +
        "    target.addEventListener('message', onMsg);\n" +
        "    target.addEventListener('error', onErr);\n" +
        "  });\n" +
        "}\n" +
        "async function startWorkers(module, memory, builder) {\n" +
        "  if (builder.numThreads() === 0) throw new Error('num_threads must be > 0');\n" +
        "  // Read this cell's glue by id (not a single shared global) so two\n" +
        "  // concurrent rayon-cell loads can't clobber each other's code.\n" +
        "  var glueCode = (self.__ironpadRayonGlue || {})[__ironpad_cell_id];\n" +
        "  // Terminate any previously-spawned pool so repeated loads of a rayon\n" +
        "  // cell don't leak a fresh set of sub-workers each time.\n" +
        "  if (self.__ironpadRayonWorkers) {\n" +
        "    self.__ironpadRayonWorkers.forEach(function(w) { w.terminate(); });\n" +
        "    self.__ironpadRayonWorkers = null;\n" +
        "  }\n" +
        "  var workerInit = {\n" +
        "    type: 'wasm_bindgen_worker_init',\n" +
        "    init: { module_or_path: module, memory: memory },\n" +
        "    receiver: builder.receiver()\n" +
        "  };\n" +
        "  var _spawned = [];\n" +
        "  try {\n" +
        "    _rayonWorkers = await Promise.all(\n" +
        "      Array.from({ length: builder.numThreads() }, async function() {\n" +
        "        var subWorkerCode =\n" +
        "          'self.onmessage = async function(e) {' +\n" +
        "          '  if (e.data && e.data.type === \"wasm_bindgen_worker_init\") {' +\n" +
        "          '    var blob = new Blob([e.data.glueCode], {type:\"application/javascript\"});' +\n" +
        "          '    var url = URL.createObjectURL(blob);' +\n" +
        "          '    try {' +\n" +
        "          '      var pkg = await import(url);' +\n" +
        "          '      await pkg.default(e.data.init);' +\n" +
        "          '      self.postMessage({type:\"wasm_bindgen_worker_ready\"});' +\n" +
        "          '      pkg.wbg_rayon_start_worker(e.data.receiver);' +\n" +
        "          '    } finally {' +\n" +
        "          '      URL.revokeObjectURL(url);' +\n" +
        "          '    }' +\n" +
        "          '  }' +\n" +
        "          '};';\n" +
        "        var subBlob = new Blob([subWorkerCode], {type:'application/javascript'});\n" +
        "        var subUrl = URL.createObjectURL(subBlob);\n" +
        "        var worker = new Worker(subUrl, {type:'module'});\n" +
        "        URL.revokeObjectURL(subUrl);\n" +
        "        _spawned.push(worker);\n" +
        "        var msg = { type: workerInit.type, init: workerInit.init,\n" +
        "                     receiver: workerInit.receiver, glueCode: glueCode };\n" +
        "        worker.postMessage(msg);\n" +
        "        await _waitForMsgType(worker, 'wasm_bindgen_worker_ready', _RAYON_WORKER_INIT_TIMEOUT_MS);\n" +
        "        return worker;\n" +
        "      })\n" +
        "    );\n" +
        "  } catch (err) {\n" +
        "    // One sub-worker failed to init: terminate every worker we spawned\n" +
        "    // so a partial pool can't leak, then reject initThreadPool.\n" +
        "    _spawned.forEach(function(w) { w.terminate(); });\n" +
        "    throw err;\n" +
        "  }\n" +
        "  self.__ironpadRayonWorkers = _rayonWorkers;\n" +
        "  builder.build();\n" +
        "}\n"
      );

      // 2) Preamble: wrap __wbg_get_imports (fallback for older wasm-bindgen).
      // 2) Preamble: wrap __wbg_get_imports (fallback for older
      //    wasm-bindgen), generated from the same env-import table.
      var _preambleExprs = _envImportExprs(globalRef, "__ironpad_cell_id");
      var preamble =
        "var __ironpad_cell_id = " + escapedCellId + ";\n" +
        "if (typeof __wbg_get_imports === 'function') {\n" +
        "  var __ironpad_orig_get_imports = __wbg_get_imports;\n" +
        "  __wbg_get_imports = function(memory) {\n" +
        "    var imports = __ironpad_orig_get_imports(memory);\n" +
        "    if (!imports.env) imports.env = {};\n" +
        Object.keys(_preambleExprs)
          .map(function (k) {
            return "    imports.env." + k + " = " + _preambleExprs[k] + ";\n";
          })
          .join("") +
        "    return imports;\n" +
        "  };\n" +
        "}\n";
      var augmentedGlue = preamble + jsGlue;
      var jsBlob = new Blob([augmentedGlue], { type: "application/javascript" });
      var jsUrl = URL.createObjectURL(jsBlob);

      // Stash the rewritten glue code, keyed by cell id, so inline startWorkers
      // (rayon thread pool) can pass it to sub-workers via postMessage. Keying
      // by id avoids a race where two concurrent bindgen loads overwrite a
      // single shared global before either's startWorkers reads it. Cleared in
      // unload().
      if (!self.__ironpadRayonGlue) self.__ironpadRayonGlue = {};
      self.__ironpadRayonGlue[cellId] = augmentedGlue;

      try {
        var mod = await import(/* webpackIgnore: true */ jsUrl);

        // Compile ahead of init so the import section can be inspected: cells
        // importing `ironpad_blocking_*` must be entered via the JSPI path in
        // execute(). wasm-bindgen's init accepts a precompiled
        // WebAssembly.Module, so compilation still happens exactly once.
        var wasmModule = await WebAssembly.compile(wasmBytes);
        var needsJspi = _moduleNeedsJspi(wasmModule);

        // wasm-bindgen's default export is the init function.
        // It returns the raw WASM exports object.
        var wasm = await mod.default({ module_or_path: wasmModule });

        // If the cell exports initThreadPool (wasm-bindgen-rayon), initialize
        // the rayon thread pool with the available hardware concurrency.
        if (typeof mod.initThreadPool === "function") {
          var concurrency = (typeof navigator !== "undefined" && navigator.hardwareConcurrency) || 4;
          await mod.initThreadPool(concurrency);
        }

        this.modules.set(cellId, {
          hash: hash,
          type: "bindgen",
          needsJspi: needsJspi, // enter via WebAssembly.promising in execute()
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
      // Materialize the SAME generated env-import table as live closures:
      // `new Function` receives rawSelf/rawCellId as parameters, so the
      // expressions close over the live executor exactly as the previous
      // hand-written object did (prototype methods delegate to the free
      // gpu helpers, and the inline Suspending guard matches _jspiImport).
      var _rawExprs = _envImportExprs("rawSelf", "rawCellId");
      var _rawBody = Object.keys(_rawExprs)
        .map(function (k) { return k + ": " + _rawExprs[k]; })
        .join(", ");
      /* eslint-disable-next-line no-new-func */
      var imports = {
        env: new Function(
          "rawSelf",
          "rawCellId",
          "return {" + _rawBody + "};"
        )(rawSelf, rawCellId),
      };
      var result = await WebAssembly.instantiate(wasmBytes, imports);
      this.modules.set(cellId, {
        hash: hash,
        type: "raw",
        needsJspi: _moduleNeedsJspi(result.module),
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
    // Ensure GPU device is initialized (no-op after first call).
    await _initGpu();

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

    // Gate blocking cells on browser support up front — a friendly error here
    // beats a Suspending-less trap from inside the instance.
    if (entry.needsJspi && !_jspiAvailable()) {
      throw new Error(JSPI_UNSUPPORTED_MSG);
    }

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
    //
    // The GPU teardown runs in `finally` so a trap in cell_main can't leave
    // `_pendingGpuReadbacks` populated or GPU handles leaked — either of which
    // would poison the *next* execute on this instance. (Interleaved executes
    // on one instance still share this state; see the note in _executeRaw.)

    var resultPtr;
    try {
      try {
        if (entry.needsJspi) {
          // Blocking cells enter through WebAssembly.promising on the RAW
          // export so the `ironpad_blocking_*` Suspending imports can park
          // the stack. The bindgen wrapper is bypassed — for sync cells it is
          // a trivial `>>> 0` shim, and blocking cells are sync by contract
          // (see ironpad_cell::blocking's module docs).
          if (typeof wasm.cell_main !== "function") {
            throw new Error("JSPI cell: missing raw 'cell_main' export");
          }
          resultPtr = await WebAssembly.promising(wasm.cell_main)(inputPtr, inputLen);
        } else {
          resultPtr = await mod.cell_main(inputPtr, inputLen);
        }
      } catch (e) {
        throw new Error(_describeWasmTrap(e, memory));
      }
      if (!resultPtr) {
        throw new Error("cell_main returned null");
      }

      // ── Read CellResult from WASM memory ───────────────────────────────

      var cellResult = this._readCellResult(memory, alloc, dealloc, resultPtr, inputPtr, inputLen, false);
      inputPtr = 0; // _readCellResult freed the input buffer — don't double-free below.

      // ── GPU post-processing ────────────────────────────────────────────

      return await this._processGpuReadbacks(cellResult);
    } finally {
      // Free the input buffer if _readCellResult never ran (trap / null return),
      // then always tear down per-run GPU state.
      if (inputPtr !== 0) dealloc(inputPtr, inputLen);
      this._pendingGpuReadbacks = [];
      _gpuCleanupHandles(Array.from(_gpuHandles.keys()));
    }
  };

  // ── Legacy raw WASM execution path ───────────────────────────────────────
  //
  // Direct WebAssembly instance access with sret calling convention detection.

  CellExecutor.prototype._executeRaw = async function (entry, inputBytes) {
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

    // Blocking cells always come from the wasm-bindgen build path; a raw
    // (legacy) blob importing them predates the feature and can't be entered
    // via promising here.
    if (entry.needsJspi) {
      if (!_jspiAvailable()) throw new Error(JSPI_UNSUPPORTED_MSG);
      throw new Error(
        "blocking host calls (ironpad_cell::blocking) require the wasm-bindgen cell build — recompile the cell"
      );
    }

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

    // GPU teardown runs in `finally` so a trap can't leave stale pending
    // readbacks or leaked handles that poison the next execute (mirrors
    // _executeBindgen). NOTE: interleaved executes on one instance still share
    // `_pendingGpuReadbacks` and the module-global handle map; fully fixing that
    // would need per-run handle scoping or serialized execution.
    try {
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
        // The sret return struct was allocated by us; free it on trap. The
        // input buffer is freed in `finally`.
        if (useSret && retptr) dealloc(retptr, CELL_RESULT_SIZE);
        throw new Error(_describeWasmTrap(e, memory));
      }

      // ── Read CellResult from WASM memory ───────────────────────────────

      var cellResult = this._readCellResult(memory, alloc, dealloc, retptr, inputPtr, inputLen, useSret);
      inputPtr = 0; // _readCellResult freed the input buffer — don't double-free below.

      // ── GPU post-processing ────────────────────────────────────────────

      return await this._processGpuReadbacks(cellResult);
    } finally {
      // Free the input buffer if _readCellResult never ran (trap / null return),
      // then always tear down per-run GPU state.
      if (inputPtr !== 0) dealloc(inputPtr, inputLen);
      this._pendingGpuReadbacks = [];
      _gpuCleanupHandles(Array.from(_gpuHandles.keys()));
    }
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
      ? new TextDecoder().decode(new Uint8Array(memory.buffer, displayPtr, displayLen).slice())
      : null;

    // Decode type tag from UTF-8.
    var typeTag = typeTagLen > 0
      ? new TextDecoder().decode(new Uint8Array(memory.buffer, typeTagPtr, typeTagLen).slice())
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
      throw new Error("WASM tick trapped: " + _describeWasmTrap(e, memory));
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
      throw new Error("WASM tick trapped: " + _describeWasmTrap(e, memory));
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

  // ── Shared LiveTickResult reader ──────────────────────────────────────

  CellExecutor.prototype._readLiveTickResult = function (
    memory, dealloc, retptr, useSret
  ) {
    var view = new DataView(memory.buffer);
    var kind = view.getUint32(retptr, true);
    var contentPtr = view.getUint32(retptr + 4, true);
    var contentLen = view.getUint32(retptr + 8, true);

    // Decode content string from UTF-8.
    var content = contentLen > 0
      ? new TextDecoder().decode(new Uint8Array(memory.buffer, contentPtr, contentLen).slice())
      : "";

    // ── Clean up WASM allocations ──────────────────────────────────────

    if (contentLen > 0) dealloc(contentPtr, contentLen);
    if (useSret || retptr) dealloc(retptr, LIVE_TICK_RESULT_SIZE);

    return { kind: kind, content: content };
  };

  // ── LiveView tick execution ───────────────────────────────────────────
  //
  // LiveView cells export `cell_tick()` which advances one step and returns
  // a LiveTickResult (12 bytes) containing the content kind and string.

  /// Execute a single tick on a loaded LiveView cell.
  ///
  /// Returns Promise<{ kind, content }>.
  CellExecutor.prototype.tickLive = async function (cellId) {
    var entry = this.modules.get(cellId);
    if (!entry) {
      throw new Error("Cell " + cellId + " not loaded");
    }

    if (entry.type === "bindgen") {
      return this._tickLiveBindgen(entry);
    } else {
      return this._tickLiveRaw(entry);
    }
  };

  // ── wasm-bindgen LiveView tick path ──────────────────────────────────

  CellExecutor.prototype._tickLiveBindgen = async function (entry) {
    var mod = entry.module;
    var wasm = entry.wasm;
    var memory = wasm.memory;
    var dealloc = wasm.ironpad_dealloc;

    if (!memory) throw new Error("wasm-bindgen module: missing 'memory' export");
    if (!dealloc) throw new Error("wasm-bindgen module: missing 'ironpad_dealloc' export");

    var tickFn = mod.cell_tick || wasm.cell_tick;
    if (!tickFn) throw new Error("Module does not export cell_tick");

    var resultPtr;
    try {
      resultPtr = await tickFn();
    } catch (e) {
      throw new Error("WASM tick trapped: " + _describeWasmTrap(e, memory));
    }

    if (!resultPtr) {
      throw new Error("cell_tick returned null");
    }

    return this._readLiveTickResult(memory, dealloc, resultPtr, false);
  };

  // ── Legacy raw LiveView tick path ────────────────────────────────────

  CellExecutor.prototype._tickLiveRaw = function (entry) {
    var instance = entry.instance;
    var memory = instance.exports.memory;
    var alloc = instance.exports.ironpad_alloc;
    var dealloc = instance.exports.ironpad_dealloc;
    var cellTick = instance.exports.cell_tick;

    if (!memory) throw new Error("raw module: missing 'memory' export");
    if (!dealloc) throw new Error("raw module: missing 'ironpad_dealloc' export");
    if (!cellTick) throw new Error("raw module: missing 'cell_tick' export");

    var retptr;
    var useSret = cellTick.length === 1;

    if (useSret) {
      if (!alloc) throw new Error("raw module: missing 'ironpad_alloc' export");
      retptr = alloc(LIVE_TICK_RESULT_SIZE);
      if (retptr === 0) {
        throw new Error("ironpad_alloc failed for live tick return struct");
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
      if (useSret && retptr) dealloc(retptr, LIVE_TICK_RESULT_SIZE);
      throw new Error("WASM tick trapped: " + _describeWasmTrap(e, memory));
    }

    return this._readLiveTickResult(memory, dealloc, retptr, useSret);
  };

  /// Remove a loaded cell module, freeing browser-side resources.
  CellExecutor.prototype.unload = function (cellId) {
    this.modules.delete(cellId);
    this._blockingPayloads.delete(cellId);
    // Release this cell's stashed rayon glue (set in loadBlob) so it isn't
    // retained for the life of the worker. No-op for non-rayon cells.
    if (self.__ironpadRayonGlue) delete self.__ironpadRayonGlue[cellId];
  };

  /// Check whether a cell has a module loaded with the given hash.
  CellExecutor.prototype.isLoaded = function (cellId, hash) {
    var existing = this.modules.get(cellId);
    return !!existing && existing.hash === hash;
  };

  /// Update a sim bus Map: get-or-create the entry for `key`, set its latest
  /// value to `json`, push to the ring buffer, and cap the ring at
  /// SIM_BUS_RING_CAP.
  ///
  /// Single source of truth for the executors that load this module: executor.js
  /// (main thread) and executor-worker.js (worker) both call this helper.
  /// executor-bridge.js does NOT — executor-core.js isn't loaded on the main
  /// thread when the bridge initializes, so the bridge keeps its own
  /// `_updateSimBus` copy (kept in sync with this one).
  CellExecutor.updateSimBus = function (bus, key, json) {
    var entry = bus.get(key);
    if (!entry) {
      entry = { latest: null, ring: [] };
      bus.set(key, entry);
    }
    entry.latest = json;
    entry.ring.push(json);
    if (entry.ring.length > SIM_BUS_RING_CAP) entry.ring.shift();
  };

  /// Write a value to the sim bus directly from the host (e.g. a slider or
  /// forwarded from main thread via sim_bus_write).
  CellExecutor.prototype.simBusWrite = function (key, value) {
    CellExecutor.updateSimBus(this._simBus, key, JSON.stringify(value));
  };

  /// Read the latest value from the sim bus (convenience for debugging).
  CellExecutor.prototype.simBusRead = function (key) {
    var entry = this._simBus.get(key);
    return entry ? JSON.parse(entry.latest) : null;
  };

  // ── Expose on the global scope ─────────────────────────────────────────────

  self.__IronpadExecutorCore = { CellExecutor: CellExecutor };
})();
