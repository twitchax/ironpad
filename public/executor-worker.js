// Web Worker entry point for ironpad cell execution.
//
// Loaded via `new Worker("/executor-worker.js")` from the main-thread bridge.
// Imports the core executor logic from worker-executor.js, wires up host
// message forwarding, and translates postMessage commands into executor calls.

"use strict";

// ── Load core executor logic ────────────────────────────────────────────────

importScripts("/worker-executor.js");

var executor = new self.CellExecutor();

// ── Host message forwarding ─────────────────────────────────────────────────
//
// WASM cells call `ironpad_host_message(ptr, len)` which lands in
// `_dispatchHostMessage`.  We intercept to read the raw JSON from WASM memory
// (only accessible here in the Worker) and forward it to the main thread.

var origDispatch = executor._dispatchHostMessage.bind(executor);

executor._dispatchHostMessage = function (cellId, ptr, len) {
  var entry = executor.modules.get(cellId);
  if (entry) {
    var memory = entry.type === "bindgen"
      ? (entry.wasm && entry.wasm.memory)
      : (entry.instance && entry.instance.exports.memory);
    if (memory) {
      var bytes = new Uint8Array(memory.buffer, ptr, len);
      var text = new TextDecoder().decode(bytes);
      self.postMessage({ type: "hostMessage", cellId: cellId, messageJson: text });
    }
  }

  // Dispatch locally as well (in case any in-worker handler is registered).
  origDispatch(cellId, ptr, len);
};

// ── Command handler ─────────────────────────────────────────────────────────
//
// Protocol:
//   Incoming:  { type: "loadBlob"|"execute"|"unload", id?, cellId, ... }
//   Outgoing:  { type: "result"|"error", id, value?|error? }
//              { type: "hostMessage", cellId, messageJson }

self.onmessage = async function (e) {
  var msg = e.data;

  if (msg.type === "loadBlob") {
    try {
      await executor.loadBlob(msg.cellId, msg.hash, msg.wasmBytes, msg.jsGlue || null);
      self.postMessage({ type: "result", id: msg.id, value: null });
    } catch (err) {
      self.postMessage({ type: "error", id: msg.id, error: err.message || String(err) });
    }
  } else if (msg.type === "execute") {
    try {
      var result = await executor.execute(msg.cellId, msg.inputBytes);
      // Transfer outputBytes buffer for zero-copy when possible.
      var transfer = result.outputBytes && result.outputBytes.buffer.byteLength > 0
        ? [result.outputBytes.buffer]
        : [];
      self.postMessage({ type: "result", id: msg.id, value: result }, transfer);
    } catch (err) {
      self.postMessage({ type: "error", id: msg.id, error: err.message || String(err) });
    }
  } else if (msg.type === "unload") {
    executor.unload(msg.cellId);
    // Fire-and-forget — no response.
  }
};
