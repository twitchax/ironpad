// Main-thread WASM executor wrapper for ironpad cells.
// Thin wrapper around executor-core.js that creates a global singleton
// (window.IronpadExecutor). Its host message handlers (progress bars, the sim
// bus) come from executor-bridge.js, its only loader, when the bridge adopts
// it as the main-thread fallback.
//
// executor-core.js must be loaded before this script (via <script> ordering).

(function () {
  "use strict";

  var CellExecutor = self.__IronpadExecutorCore.CellExecutor;

  // The injected WASM glue reaches the executor for FFI callbacks (sim_read,
  // GPU, host messages) via this global-reference string. The bridge reclaims
  // `window.IronpadExecutor` for itself after loading this fallback, so the
  // fallback must own a DISTINCT global (`window.__IronpadFallback`) — otherwise
  // fallback cells' FFI shims call the bridge, which has no _simRead/_gpu*.
  var executor = new CellExecutor("window.__IronpadFallback");

  // Stable global for the FFI shims (matches the globalRef above). This one
  // persists; the bridge only reclaims `window.IronpadExecutor`.
  window.__IronpadFallback = executor;
  // The bridge grabs this immediately after load, then restores itself.
  window.IronpadExecutor = executor;
})();
