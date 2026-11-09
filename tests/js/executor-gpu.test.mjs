/**
 * GPU canvas runs through the executor (executor-gpu.js + executor-core.js).
 *
 *   cargo make test-js
 *
 * A fake WebGPU device stands behind `navigator.gpu`, injected into the real
 * executor-gpu.js, and a cell's cell_main is plain JS that calls the same
 * executor imports `GpuCanvas::render` does: three buffers, one dispatch, a
 * `gpu_read_pixels` host message, then a CellResult with a placeholder canvas.
 */

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";

import { loadCore, makeMemory, rawEntry } from "./executor-harness.mjs";

/**
 * A fake WebGPU behind `navigator.gpu`. Every dispatch "renders" `pixel`
 * (RGBA floats) into the bound output buffer; `scopeError` is what each
 * validation scope pops with (null for a valid shader).
 */
function fakeGpu({ scopeError = null, pixel = [1, 0.5, 0, 1] } = {}) {
  const log = { pushed: 0, popped: 0 };
  class FakeBuffer {
    constructor({ size }) {
      this.data = new Float32Array(Math.ceil(size / 4));
    }
    destroy() {}
    async mapAsync() {}
    getMappedRange() {
      return this.data.buffer;
    }
    unmap() {}
  }
  let bound = null;
  const device = {
    lost: new Promise(() => {}),
    queue: { writeBuffer() {}, submit() {} },
    createBuffer: (desc) => new FakeBuffer(desc),
    createShaderModule: () => ({}),
    createBindGroupLayout: () => ({}),
    createBindGroup: (desc) => desc,
    createPipelineLayout: () => ({}),
    createComputePipeline: () => ({}),
    createCommandEncoder: () => ({
      beginComputePass: () => ({
        setPipeline() {},
        setBindGroup(_i, group) {
          bound = group;
        },
        dispatchWorkgroups() {
          const out = bound.entries[1].resource.buffer.data;
          for (let i = 0; i < out.length; i += 4) out.set(pixel, i);
        },
        end() {},
      }),
      copyBufferToBuffer(src, _so, dst) {
        dst.data.set(src.data);
      },
      finish: () => ({}),
    }),
    pushErrorScope() {
      log.pushed += 1;
    },
    popErrorScope() {
      log.popped += 1;
      return Promise.resolve(scopeError);
    },
  };
  const adapter = {
    limits: { maxStorageBufferBindingSize: 1 << 27, maxBufferSize: 1 << 28 },
    requestDevice: async () => device,
  };
  const globals = {
    navigator: { gpu: { requestAdapter: async () => adapter } },
    GPUBufferUsage: { MAP_READ: 1, COPY_SRC: 4, COPY_DST: 8, STORAGE: 128 },
    GPUShaderStage: { COMPUTE: 4 },
    GPUMapMode: { READ: 1 },
  };
  return { globals, log };
}

const BLOB_PANEL = (w, h, count = 1) =>
  JSON.stringify(
    Array.from({ length: count }, () => ({
      BlobImage: { mime_type: "image/bmp", base64_data: "", width: w, height: h },
    })),
  );

/**
 * A raw cell whose cell_main renders `count` `width` x `height` GpuCanvases
 * the way the Rust side does, and returns `typeTag` with `outputBytes` as its
 * value. `trap` makes it throw after dispatching, as a cell that panics would.
 */
function gpuCell(
  executor,
  cell,
  { width = 2, height = 1, count = 1, typeTag = "Canvas", outputBytes = [9], trap = false } = {},
) {
  return rawEntry(cell, {
    cell_main() {
      for (let i = 0; i < count; i += 1) {
        const uniform = executor._gpuCreateBuffer(8, 3);
        const output = executor._gpuCreateBuffer(width * height * 16, 1);
        const staging = executor._gpuCreateBuffer(width * height * 16, 2);
        const shader = cell.putText(1024, "@compute fn main() {}");
        const status = executor._gpuDispatchComputeForCell("c", shader.ptr, shader.len, uniform, output, width, height);
        assert.equal(status, 0, "a dispatch the device accepted reports success");
        const msg = cell.putText(
          2048,
          JSON.stringify({ type: "gpu_read_pixels", output_handle: output, staging_handle: staging, width, height }),
        );
        executor._dispatchHostMessage("c", msg.ptr, msg.len);
      }
      if (trap) throw new Error("unreachable");
      return writeCellResult(cell, outputBytes, BLOB_PANEL(width, height, count), typeTag);
    },
  });
}

/** A plain cell with no GPU work, returning text. */
function textCell(cell) {
  return rawEntry(cell, {
    cell_main: () => writeCellResult(cell, [], '[{"Text":"ok"}]', "String"),
  });
}

/** A CellResult at 256 over output bytes, display text and a type tag. */
function writeCellResult(cell, outputBytes, display, typeTag) {
  cell.put(3000, outputBytes);
  const d = cell.putText(4000, display);
  const t = cell.putText(5000, typeTag);
  return cell.putU32s(256, [3000, outputBytes.length, d.ptr, d.len, t.ptr, t.len]);
}

// ── Shader validation errors (review bug-cell-10) ───────────────────────────

test("an invalid shader fails the cell with the GPU's message", async () => {
  // WebGPU reports WGSL and pipeline errors on an error scope, never by
  // throwing, so the dispatch "succeeded" and the cell showed the untouched
  // zero-filled buffer as a black image.
  const { globals } = fakeGpu({ scopeError: { message: "bad wgsl" } });
  const { executor } = loadCore({ globals });
  const cell = makeMemory();
  executor.modules.set("c", gpuCell(executor, cell));

  await assert.rejects(executor.execute("c", new Uint8Array(0)), /GPU shader error: bad wgsl/);
});

test("a valid shader's readback fills the panel", async () => {
  const { globals, log } = fakeGpu();
  const { executor } = loadCore({ globals });
  const cell = makeMemory();
  executor.modules.set("c", gpuCell(executor, cell));

  const result = await executor.execute("c", new Uint8Array(0));

  const [panel] = JSON.parse(result.displayText);
  assert.ok(panel.BlobImage.base64_data.length > 0);
  assert.equal(log.pushed, log.popped, "every scope pushed was popped");
});

test("a cell that trapped after a bad dispatch does not hand its error to the next run", async () => {
  const { globals } = fakeGpu({ scopeError: { message: "bad wgsl" } });
  const { executor } = loadCore({ globals });
  const cell = makeMemory();
  executor.modules.set("c", gpuCell(executor, cell, { trap: true }));
  await assert.rejects(executor.execute("c", new Uint8Array(0)), /trapped/);

  executor.modules.set("c", textCell(cell));
  const result = await executor.execute("c", new Uint8Array(0));
  assert.equal(result.displayText, '[{"Text":"ok"}]');
});

test("a dispatch with an unknown buffer handle fails without opening a scope", () => {
  const { globals, log } = fakeGpu();
  const { executor, g } = loadCore({ globals });
  const cell = makeMemory();
  executor.modules.set("c", rawEntry(cell));
  return g.__IronpadExecutorGpu.init().then(() => {
    const shader = cell.putText(1024, "@compute fn main() {}");
    assert.equal(executor._gpuDispatchComputeForCell("c", shader.ptr, shader.len, 999, 998, 2, 1), 1);
    assert.equal(log.pushed, 0);
  });
});

test("a readback that comes back empty fails the cell instead of leaving the placeholder", async () => {
  const { globals } = fakeGpu();
  const { executor } = loadCore({ globals });
  const cell = makeMemory();
  executor.modules.set(
    "c",
    rawEntry(cell, {
      cell_main() {
        const msg = cell.putText(
          2048,
          JSON.stringify({ type: "gpu_read_pixels", output_handle: 77, staging_handle: 78, width: 2, height: 1 }),
        );
        executor._dispatchHostMessage("c", msg.ptr, msg.len);
        return writeCellResult(cell, [9], BLOB_PANEL(2, 1), "Canvas");
      },
    }),
  );

  await assert.rejects(executor.execute("c", new Uint8Array(0)), /GPU readback failed/);
});

// ── The piped value of a GpuCanvas (review bug-cell-11) ─────────────────────

const GOLDEN = readFileSync(
  path.join(
    path.dirname(fileURLToPath(import.meta.url)),
    "..",
    "..",
    "crates",
    "ironpad-cell",
    "tests",
    "golden",
    "canvas_300x1.bincode",
  ),
);

/** canvas.rs's `golden_pixel`, column by column. */
function goldenRgb(width) {
  const rgb = new Uint8Array(width * 3);
  for (let x = 0; x < width; x += 1) {
    const b = x & 0xff;
    rgb.set([b, (b * 7) & 0xff, (b * 13) & 0xff], x * 3);
  }
  return rgb;
}

test("encodeCanvasBincode matches the crate's own encoding of a Canvas", () => {
  // The golden is bincode::serde::encode_to_vec of a 300x1 Canvas, written
  // by ironpad-cell's canvas_bincode_matches_the_golden.
  const { g } = loadCore();
  const bytes = g.__IronpadExecutorGpu.encodeCanvasBincode(300, 1, goldenRgb(300));

  assert.deepEqual(Buffer.from(bytes), GOLDEN);
});

test("encodeCanvasBincode switches to the four-byte varint past u16", () => {
  // bincode's varint: < 251 one byte, then 251 + u16, 252 + u32, 253 + u64.
  const { g } = loadCore();
  const bytes = g.__IronpadExecutorGpu.encodeCanvasBincode(70000, 1, new Uint8Array(210000));

  assert.deepEqual(Array.from(bytes.subarray(0, 11)), [252, 0x70, 0x11, 0x01, 0x00, 1, 252, 0x50, 0x34, 0x03, 0x00]);
  assert.equal(bytes.length, 11 + 210000);
});

test("a GpuCanvas pipes the rendered pixels, not its gray placeholder", async () => {
  // The cell's value was the bincode of the placeholder render() returns on
  // the GPU path, so the image showed right while downstream cells read gray.
  const { globals } = fakeGpu({ pixel: [1, 0.5, 0, 1] });
  const { executor } = loadCore({ globals });
  const cell = makeMemory();
  executor.modules.set("c", gpuCell(executor, cell, { width: 2, height: 1 }));

  const result = await executor.execute("c", new Uint8Array(0));

  assert.deepEqual(Array.from(result.outputBytes), [2, 1, 6, 255, 128, 0, 255, 128, 0]);
});

test("the piped value is left alone when it is not a lone Canvas", async () => {
  // Another tag (a tuple holding the canvas, say) cannot be patched this way,
  // and with two dispatches the returned canvas cannot be told apart.
  for (const options of [{ typeTag: "(Table, Canvas)" }, { typeTag: "Canvas", count: 2 }]) {
    const { globals } = fakeGpu();
    const { executor } = loadCore({ globals });
    const cell = makeMemory();
    executor.modules.set("c", gpuCell(executor, cell, options));

    const result = await executor.execute("c", new Uint8Array(0));

    assert.deepEqual(Array.from(result.outputBytes), [9], JSON.stringify(options));
  }
});

