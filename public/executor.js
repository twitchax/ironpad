// Main-thread WASM executor wrapper for ironpad cells.
// Thin wrapper around executor-core.js that creates a global singleton
// (window.IronpadExecutor) and registers DOM-dependent host message handlers.
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

  // ── Built-in host message handlers ──────────────────────────────────────
  //
  // NOTE: These built-in handlers are intentionally duplicated in
  // executor-bridge.js. The bridge can't load executor-core.js on the main
  // thread at init time, so there is no shared home for them — keep the two
  // copies in sync.

  executor.onHostMessage("progress_update", function (msg, _cellId) {
    var el = document.querySelector('[data-progress-id="' + msg.id + '"]');
    if (!el) return;

    // Harden the host-message boundary: a non-numeric value would render
    // "NaN%" and `width: NaN%`. Fall back to 0 when it isn't a finite number.
    var value = Number.isFinite(msg.value) ? msg.value : 0;

    var fill = el.querySelector(".ironpad-progress-fill");
    if (fill) {
      var pct = Math.min(100, Math.max(0, value));
      fill.style.width = pct + "%";
    }

    var label = el.querySelector(".ironpad-progress-value");
    if (label) {
      label.textContent = Math.round(value) + "%";
    }
  });

  executor.onHostMessage("sim_emit", function (msg, _cellId) {
    CellExecutor.updateSimBus(executor._simBus, msg.key, JSON.stringify(msg.value));
  });

  // Stable global for the FFI shims (matches the globalRef above). This one
  // persists; the bridge only reclaims `window.IronpadExecutor`.
  window.__IronpadFallback = executor;
  // The bridge grabs this immediately after load, then restores itself.
  window.IronpadExecutor = executor;
})();
