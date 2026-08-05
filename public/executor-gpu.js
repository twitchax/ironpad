// WebGPU runtime for ironpad cells: capability probing, device and handle
// management, compute dispatch, async readback, and BMP construction.
//
// Split out of executor-core.js (PRD-0055 T-002). Loads in BOTH contexts
// before executor-core.js: the worker chain importScripts it
// (executor-worker-core.js) and the main-thread fallback injects it
// (executor-bridge.js). Everything is exposed on
// `self.__IronpadExecutorGpu`; executor-core.js is the only consumer.

(function () {
  "use strict";

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


  // ── Expose on the global scope ─────────────────────────────────────────────

  self.__IronpadExecutorGpu = {
    init: _initGpu,
    availableSync: _gpuAvailableSync,
    createBuffer: _gpuCreateBuffer,
    writeBuffer: _gpuWriteBuffer,
    dispatchComputeSync: _gpuDispatchComputeSync,
    readPixels: _gpuReadPixels,
    buildBmp: _gpuBuildBmp,
    cleanupAllHandles: function () {
      _gpuCleanupHandles(Array.from(_gpuHandles.keys()));
    },
  };
})();
