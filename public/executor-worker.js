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

// ── DOM shim for wasm text measurement ──────────────────────────────────────
//
// plotters (and anything else compiled for wasm32-unknown-unknown) measures
// text through the DOM: web_sys::window() → document → append a styled <span>,
// read its offsetWidth/offsetHeight (plotters src/style/font/web.rs — the only
// font backend plotters offers on wasm32). Workers have no DOM, so that path
// panics and every text-bearing plot cell used to bounce to the slow
// main-thread fallback. Provide exactly the surface that measurement path
// touches, backed by OffscreenCanvas.measureText (which workers DO have).
//
// web_sys::window() is js_sys::global().dyn_into::<Window>(), and the glue's
// instanceof check consults the global `Window` constructor (inside try/catch,
// so "undefined" reads as false). Pointing `Window` at the worker global's own
// constructor makes the check pass, and "window" becomes this worker scope —
// whose fetch/setTimeout/etc. are the real functions, so crates like reqwest
// that prefer the Window branch keep working unchanged.

(function () {
  var measureCtx = null;

  function context2d() {
    if (measureCtx === null) {
      measureCtx = new OffscreenCanvas(1, 1).getContext("2d");
    }
    return measureCtx;
  }

  function MeasureSpan() {
    this._text = "";
    this._family = "sans-serif";
    this._style = "normal";
    this._size = 16;
  }

  Object.defineProperty(MeasureSpan.prototype, "textContent", {
    get: function () {
      return this._text;
    },
    set: function (value) {
      this._text = value == null ? "" : String(value);
    },
  });

  MeasureSpan.prototype.setAttribute = function (name, value) {
    if (name !== "style") return;
    // plotters emits: "display: inline-block; font-family:{f}; font-style:{s};
    // font-size: {n}px; position: fixed; top: 100%".
    var family = /font-family:\s*([^;]+)/.exec(value);
    var style = /font-style:\s*([^;]+)/.exec(value);
    var size = /font-size:\s*([0-9.]+)px/.exec(value);
    if (family) this._family = family[1].trim();
    if (style) this._style = style[1].trim();
    if (size) this._size = parseFloat(size[1]);
  };

  MeasureSpan.prototype._measure = function () {
    var ctx = context2d();
    // Canvas font shorthand: [style|weight] size family. plotters reuses
    // font-style for its Bold variant, which CSS calls a weight — both are
    // valid in the leading slot, so pass it through unless it's "normal".
    var prefix = this._style !== "normal" ? this._style + " " : "";
    ctx.font = prefix + this._size + "px " + this._family;
    // A DOM span collapses newlines to spaces; mirror that.
    return ctx.measureText(this._text.replace(/\n/g, " "));
  };

  Object.defineProperty(MeasureSpan.prototype, "offsetWidth", {
    get: function () {
      return Math.ceil(this._measure().width);
    },
  });

  Object.defineProperty(MeasureSpan.prototype, "offsetHeight", {
    get: function () {
      var metrics = this._measure();
      var ascent = metrics.fontBoundingBoxAscent;
      var descent = metrics.fontBoundingBoxDescent;
      if (typeof ascent === "number" && typeof descent === "number") {
        return Math.ceil(ascent + descent);
      }
      return Math.ceil(this._size * 1.2);
    },
  });

  MeasureSpan.prototype.remove = function () {};

  // dyn_into::<Window>() / dyn_into::<HtmlElement>() instanceof targets.
  self.Window = self.constructor;
  self.HTMLElement = MeasureSpan;

  self.document = {
    body: {
      // body.append_with_node_1(&span) → body.append(span): attach is a no-op,
      // the span measures itself lazily via the offset getters.
      append: function () {},
    },
    createElement: function () {
      return new MeasureSpan();
    },
  };
})();

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
