// Main-thread WASM executor wrapper for ironpad cells.
// Thin wrapper around executor-core.js that creates a global singleton
// (window.IronpadExecutor) and registers DOM-dependent host message handlers.
//
// executor-core.js must be loaded before this script (via <script> ordering).

(function () {
  "use strict";

  var CellExecutor = self.__IronpadExecutorCore.CellExecutor;

  var executor = new CellExecutor("window.IronpadExecutor");

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

  executor.onHostMessage("sim_emit", function (msg, _cellId) {
    var key = msg.key;
    var json = JSON.stringify(msg.value);
    var entry = executor._simBus.get(key);
    if (!entry) {
      entry = { latest: null, ring: [] };
      executor._simBus.set(key, entry);
    }
    entry.latest = json;
    entry.ring.push(json);
    if (entry.ring.length > 1000) entry.ring.shift();
  });

  window.IronpadExecutor = executor;
})();
