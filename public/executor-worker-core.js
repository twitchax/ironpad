// Worker-side WASM executor wrapper for ironpad cells.
// Thin wrapper around executor-core.js that exposes CellExecutor on the Worker
// global.  executor-worker.js imports this via importScripts and creates the
// executor instance.
//
// executor-core.js is loaded via importScripts (no <script> ordering in Workers).

"use strict";

importScripts("/executor-core.js" + (self.location.search || ""));

var _CoreCellExecutor = self.__IronpadExecutorCore.CellExecutor;

// Wrap the core constructor so it defaults globalRef and stashes the instance
// on the Worker global where dynamically-imported ESM glue modules expect it.
self.CellExecutor = function () {
  var inst = new _CoreCellExecutor("self._ironpadExecutor");
  self._ironpadExecutor = inst;
  return inst;
};
