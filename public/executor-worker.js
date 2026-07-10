// Web Worker entry point for ironpad cell execution.
//
// Loaded via `new Worker("/executor-worker.js")` from the main-thread bridge.
// Imports the core executor logic from worker-executor.js, wires up host
// message forwarding, and translates postMessage commands into executor calls.

"use strict";

// ── Panic message capture ───────────────────────────────────────────────────
//
// When WASM panics, console_error_panic_hook logs the real message to
// console.error, but the JS catch only sees a generic "unreachable" trap.
// We intercept console.error to capture the last panic message so we can
// include it in the error sent back to the main thread.

var _lastPanicMessage = null;
var _origConsoleError = console.error;
console.error = function () {
  var msg = Array.prototype.join.call(arguments, " ");
  if (msg.indexOf("panicked at") !== -1) {
    _lastPanicMessage = msg;
  }
  _origConsoleError.apply(console, arguments);
};

// ── Load core executor logic ────────────────────────────────────────────────

importScripts("/worker-executor.js");

var executor = new self.CellExecutor();
var _updateSimBus = self.__IronpadExecutorCore.CellExecutor.updateSimBus;

// ── Sim bus: update local bus on sim_emit ────────────────────────────────────
//
// sim_emit messages arrive via _dispatchHostMessage (forwarded from WASM).
// The worker's bus must be updated so ironpad_sim_read can serve reads from
// simulation cells running in this worker.

executor.onHostMessage("sim_emit", function (msg, _cellId) {
  _updateSimBus(executor._simBus, msg.key, JSON.stringify(msg.value));
});

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
//   Incoming:  { type: "loadBlob"|"execute"|"tick"|"tick_live"|"unload"|"sim_bus_write", id?, cellId, ... }
//   Outgoing:  { type: "result"|"error", id, value?|error? }
//              { type: "hostMessage", cellId, messageJson }

// Run an awaited executor op, correlating any WASM panic captured by the patched
// console.error to THIS op. console.error is global, so a captured panic can't
// carry a request id; resetting `_lastPanicMessage` immediately before the call
// and consuming it in the catch keeps a panic from a *prior* op from bleeding
// into an unrelated error message. (Two genuinely-interleaved async cells still
// share this single global — a fully race-free fix would serialize onmessage.)
async function runWithPanicCapture(op) {
  _lastPanicMessage = null;
  try {
    return await op();
  } catch (err) {
    throw new Error(_lastPanicMessage || err.message || String(err));
  } finally {
    _lastPanicMessage = null;
  }
}

self.onmessage = async function (e) {
  var msg = e.data;

  if (msg.type === "sim_bus_write") {
    // Main thread (e.g. a slider) wrote a value to the bus — mirror it here
    // so simulation cells running in this worker can read it via ironpad_sim_read.
    _updateSimBus(executor._simBus, msg.key, msg.json);
    return; // No response needed.
  }

  if (msg.type === "loadBlob") {
    try {
      await runWithPanicCapture(function () {
        return executor.loadBlob(msg.cellId, msg.hash, msg.wasmBytes, msg.jsGlue || null);
      });
      self.postMessage({ type: "result", id: msg.id, value: null });
    } catch (err) {
      self.postMessage({ type: "error", id: msg.id, error: err.message });
    }
  } else if (msg.type === "execute") {
    try {
      var result = await runWithPanicCapture(function () {
        return executor.execute(msg.cellId, msg.inputBytes);
      });
      // Transfer outputBytes buffer for zero-copy when possible.
      var transfer = result.outputBytes && result.outputBytes.buffer.byteLength > 0
        ? [result.outputBytes.buffer]
        : [];
      self.postMessage({ type: "result", id: msg.id, value: result }, transfer);
    } catch (err) {
      self.postMessage({ type: "error", id: msg.id, error: err.message });
    }
  } else if (msg.type === "tick") {
    try {
      var result = await runWithPanicCapture(function () {
        return executor.tick(msg.cellId);
      });
      // Transfer rgbBytes buffer for zero-copy when possible.
      var transfer = result.rgbBytes && result.rgbBytes.buffer.byteLength > 0
        ? [result.rgbBytes.buffer]
        : [];
      self.postMessage({ type: "result", id: msg.id, value: result }, transfer);
    } catch (err) {
      self.postMessage({ type: "error", id: msg.id, error: err.message });
    }
  } else if (msg.type === "tick_live") {
    try {
      var result = await runWithPanicCapture(function () {
        return executor.tickLive(msg.cellId);
      });
      self.postMessage({ type: "result", id: msg.id, value: result });
    } catch (err) {
      self.postMessage({ type: "error", id: msg.id, error: err.message });
    }
  } else if (msg.type === "unload") {
    executor.unload(msg.cellId);
    // Fire-and-forget — no response.
  }
};
