// Worker-side WASM executor wrapper for ironpad cells.
// Thin wrapper around executor-core.js that exposes CellExecutor on the Worker
// global.  executor-worker.js imports this via importScripts and creates the
// executor instance.
//
// The executor scripts load via importScripts (no <script> ordering in
// Workers). Order matters: executor-core.js reads the Gpu/Glue namespaces
// its two companions register (PRD-0055 T-002).

"use strict";

importScripts("/executor-gpu.js" + (self.location.search || ""));
importScripts("/executor-glue.js" + (self.location.search || ""));
importScripts("/executor-core.js" + (self.location.search || ""));

var _CoreCellExecutor = self.__IronpadExecutorCore.CellExecutor;

// Wrap the core constructor so it defaults globalRef and stashes the instance
// on the Worker global where dynamically-imported ESM glue modules expect it.
self.CellExecutor = function () {
  var inst = new _CoreCellExecutor("self._ironpadExecutor");
  self._ironpadExecutor = inst;
  return inst;
};
