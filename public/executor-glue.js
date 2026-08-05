// wasm-bindgen glue rewriting for ironpad cells: the env host-import table,
// the ESM 'env' star-import shim, the inline rayon startWorkers codegen, and
// the __wbg_get_imports preamble.
//
// Split out of executor-core.js (PRD-0055 T-002). Loads in BOTH contexts
// before executor-core.js (worker importScripts chain; main-thread fallback
// injection). Everything here is TEXT TRANSFORMATION — pure functions from
// glue strings to glue strings — exposed on `self.__IronpadExecutorGlue`.
//
// The env-import table must list exactly the imports declared by
// ironpad-cell's `#[link(wasm_import_module = "env")]` extern blocks; a unit
// test in ironpad-app (compiler/build.rs) asserts the two sets match.

(function () {
  "use strict";

  var JSPI_UNSUPPORTED_MSG =
    "This cell uses blocking host calls (ironpad_cell::blocking), which need " +
    "WebAssembly JSPI: available in Chrome and Edge 137+, not yet in Firefox " +
    "or Safari.";

  // ── env host-import table ─────────────────────────────────────────────
  //
  // THE single source of truth for the `env` imports a cell's WASM module
  // may declare. The authoritative Rust side is the `#[link(wasm_import_module
  // = "env")] extern "C"` blocks in ironpad-cell (lib.rs, sim.rs, gpu.rs,
  // blocking.rs); this table must list exactly those names.
  //
  // Each entry builds the import's JS function EXPRESSION as a string,
  // parameterized by:
  //   - g:   an expression evaluating to the executor object;
  //   - cid: an expression evaluating to the cell id.
  // Three consumers generate from it: the ESM `import * from 'env'` rewrite,
  // the `__wbg_get_imports` preamble (older wasm-bindgen fallback), and the
  // raw-WASM imports object. They used to be three hand-written copies, and
  // a new import missing from ONE copy failed instantiation only on that
  // specific loading path — which no test exercises.
  function _envImportExprs(g, cid) {
    // The Suspending wrappers are built inline (never via the executor
    // global): the generated text also evaluates inside rayon sub-workers,
    // where the executor global is undefined — a build-time dereference
    // would crash worker init before the thread pool exists.
    function jspiGuarded(fnExpr) {
      return (
        "(typeof WebAssembly.Suspending === 'function') " +
        "? new WebAssembly.Suspending(" + fnExpr + ") " +
        ": function() { throw new Error(" + JSON.stringify(JSPI_UNSUPPORTED_MSG) + "); }"
      );
    }
    return {
      ironpad_host_message:
        "function(ptr, len) { if (" + g + ") { " + g + "._dispatchHostMessage(" + cid + ", ptr, len); } }",
      ironpad_sim_read:
        "function(ptr, len) { return " + g + " ? " + g + "._simRead(" + cid + ", ptr, len) : 0; }",
      ironpad_sim_read_all:
        "function(ptr, len) { return " + g + " ? " + g + "._simReadAll(" + cid + ", ptr, len) : 0; }",
      ironpad_gpu_available:
        "function() { return " + g + "._gpuAvailableSync(); }",
      ironpad_gpu_create_buffer:
        "function(s, u) { return " + g + "._gpuCreateBuffer(s, u); }",
      ironpad_gpu_write_buffer:
        "function(h, p, l) { if (" + g + ") " + g + "._gpuWriteBufferForCell(" + cid + ", h, p, l); }",
      ironpad_gpu_dispatch_compute:
        "function(sp, sl, uh, oh, w, h) { return " + g + " ? " + g + "._gpuDispatchComputeForCell(" + cid + ", sp, sl, uh, oh, w, h) : 1; }",
      ironpad_blocking_sleep_ms: jspiGuarded(
        "function(ms) { return new Promise(function(res) { setTimeout(res, ms); }); }"
      ),
      ironpad_blocking_fetch: jspiGuarded(
        "function(p, l) { return " + g + "._blockingFetch(" + cid + ", p, l); }"
      ),
      ironpad_blocking_fetch_ok:
        "function() { return " + g + " ? " + g + "._blockingFetchOk(" + cid + ") : 0; }",
      ironpad_blocking_read:
        "function(p, l) { return " + g + " ? " + g + "._blockingRead(" + cid + ", p, l) : 0; }",
    };
  }

  /// Rewrite wasm-bindgen ESM glue so it loads from a blob URL: replace the
  /// bare `import * from 'env'` with an inline shim, replace the
  /// wasm-bindgen-rayon workerHelpers import with an inline startWorkers,
  /// and prepend the `__wbg_get_imports` preamble (older-bindgen fallback).
  /// Returns the augmented glue text; the caller owns blob creation and the
  /// per-cell rayon-glue stash. (Body indentation is inherited verbatim from
  /// its previous home inside loadBlob.)
  function rewriteBindgenGlue(jsGlue, cellId, globalRef) {
      var escapedCellId = JSON.stringify(cellId);

      // 1) Replace bare `import * as __wbg_starN from 'env'` with an
      //    inline shim so the ESM can load from a blob URL.
      // Generated from the single env-import table above.
      var _shimExprs = _envImportExprs(globalRef, escapedCellId);
      var hostShimBody = Object.keys(_shimExprs)
        .map(function (k) { return k + ": " + _shimExprs[k]; })
        .join(", ");

      jsGlue = jsGlue.replace(
        /import\s*\*\s*as\s+(\w+)\s+from\s+['"]env['"]\s*;?/g,
        function (_match, starName) {
          return "var " + starName + " = { " + hostShimBody + " };";
        }
      );

      // 1b) Replace wasm-bindgen-rayon snippet imports with an inline
      //     startWorkers implementation.  The snippet lives at a relative
      //     path like `./snippets/wasm-bindgen-rayon-HASH/src/workerHelpers.js`
      //     which cannot be resolved when the glue is loaded from a blob URL.
      //
      //     Our inline version creates rayon sub-workers from blob URLs,
      //     passing the rewritten glue code so each sub-worker can import
      //     it and call `pkg.default(init)` + `pkg.wbg_rayon_start_worker`.
      jsGlue = jsGlue.replace(
        /import\s*\{\s*startWorkers\s*\}\s+from\s+['"][^'"]*wasm-bindgen-rayon[^'"]*workerHelpers\.js['"];?/g,
        "\n" +
        "var _rayonWorkers;\n" +
        "var _RAYON_WORKER_INIT_TIMEOUT_MS = 30000;\n" +
        "// Resolve when `target` posts a message of `type`; reject on worker\n" +
        "// error or after `timeoutMs`. Both listeners AND the timer are always\n" +
        "// torn down so a sub-worker that never signals ready can't leak the\n" +
        "// listener or hang initThreadPool forever.\n" +
        "function _waitForMsgType(target, type, timeoutMs) {\n" +
        "  return new Promise(function(resolve, reject) {\n" +
        "    function cleanup() {\n" +
        "      clearTimeout(timer);\n" +
        "      target.removeEventListener('message', onMsg);\n" +
        "      target.removeEventListener('error', onErr);\n" +
        "    }\n" +
        "    var timer = setTimeout(function() {\n" +
        "      cleanup();\n" +
        "      reject(new Error('rayon worker init timed out after ' + timeoutMs + 'ms'));\n" +
        "    }, timeoutMs);\n" +
        "    function onMsg(event) {\n" +
        "      if (!event.data || event.data.type !== type) return;\n" +
        "      cleanup();\n" +
        "      resolve(event.data);\n" +
        "    }\n" +
        "    function onErr(event) {\n" +
        "      cleanup();\n" +
        "      reject(new Error('rayon worker failed to initialize: ' + (event.message || 'unknown error')));\n" +
        "    }\n" +
        "    target.addEventListener('message', onMsg);\n" +
        "    target.addEventListener('error', onErr);\n" +
        "  });\n" +
        "}\n" +
        "async function startWorkers(module, memory, builder) {\n" +
        "  if (builder.numThreads() === 0) throw new Error('num_threads must be > 0');\n" +
        "  // Read this cell's glue by id (not a single shared global) so two\n" +
        "  // concurrent rayon-cell loads can't clobber each other's code.\n" +
        "  var glueCode = (self.__ironpadRayonGlue || {})[__ironpad_cell_id];\n" +
        "  // Terminate any previously-spawned pool so repeated loads of a rayon\n" +
        "  // cell don't leak a fresh set of sub-workers each time.\n" +
        "  if (self.__ironpadRayonWorkers) {\n" +
        "    self.__ironpadRayonWorkers.forEach(function(w) { w.terminate(); });\n" +
        "    self.__ironpadRayonWorkers = null;\n" +
        "  }\n" +
        "  var workerInit = {\n" +
        "    type: 'wasm_bindgen_worker_init',\n" +
        "    init: { module_or_path: module, memory: memory },\n" +
        "    receiver: builder.receiver()\n" +
        "  };\n" +
        "  var _spawned = [];\n" +
        "  try {\n" +
        "    _rayonWorkers = await Promise.all(\n" +
        "      Array.from({ length: builder.numThreads() }, async function() {\n" +
        "        var subWorkerCode =\n" +
        "          'self.onmessage = async function(e) {' +\n" +
        "          '  if (e.data && e.data.type === \"wasm_bindgen_worker_init\") {' +\n" +
        "          '    var blob = new Blob([e.data.glueCode], {type:\"application/javascript\"});' +\n" +
        "          '    var url = URL.createObjectURL(blob);' +\n" +
        "          '    try {' +\n" +
        "          '      var pkg = await import(url);' +\n" +
        "          '      await pkg.default(e.data.init);' +\n" +
        "          '      self.postMessage({type:\"wasm_bindgen_worker_ready\"});' +\n" +
        "          '      pkg.wbg_rayon_start_worker(e.data.receiver);' +\n" +
        "          '    } finally {' +\n" +
        "          '      URL.revokeObjectURL(url);' +\n" +
        "          '    }' +\n" +
        "          '  }' +\n" +
        "          '};';\n" +
        "        var subBlob = new Blob([subWorkerCode], {type:'application/javascript'});\n" +
        "        var subUrl = URL.createObjectURL(subBlob);\n" +
        "        var worker = new Worker(subUrl, {type:'module'});\n" +
        "        URL.revokeObjectURL(subUrl);\n" +
        "        _spawned.push(worker);\n" +
        "        var msg = { type: workerInit.type, init: workerInit.init,\n" +
        "                     receiver: workerInit.receiver, glueCode: glueCode };\n" +
        "        worker.postMessage(msg);\n" +
        "        await _waitForMsgType(worker, 'wasm_bindgen_worker_ready', _RAYON_WORKER_INIT_TIMEOUT_MS);\n" +
        "        return worker;\n" +
        "      })\n" +
        "    );\n" +
        "  } catch (err) {\n" +
        "    // One sub-worker failed to init: terminate every worker we spawned\n" +
        "    // so a partial pool can't leak, then reject initThreadPool.\n" +
        "    _spawned.forEach(function(w) { w.terminate(); });\n" +
        "    throw err;\n" +
        "  }\n" +
        "  self.__ironpadRayonWorkers = _rayonWorkers;\n" +
        "  builder.build();\n" +
        "}\n"
      );

      // 2) Preamble: wrap __wbg_get_imports (fallback for older wasm-bindgen).
      // 2) Preamble: wrap __wbg_get_imports (fallback for older
      //    wasm-bindgen), generated from the same env-import table.
      var _preambleExprs = _envImportExprs(globalRef, "__ironpad_cell_id");
      var preamble =
        "var __ironpad_cell_id = " + escapedCellId + ";\n" +
        "if (typeof __wbg_get_imports === 'function') {\n" +
        "  var __ironpad_orig_get_imports = __wbg_get_imports;\n" +
        "  __wbg_get_imports = function(memory) {\n" +
        "    var imports = __ironpad_orig_get_imports(memory);\n" +
        "    if (!imports.env) imports.env = {};\n" +
        Object.keys(_preambleExprs)
          .map(function (k) {
            return "    imports.env." + k + " = " + _preambleExprs[k] + ";\n";
          })
          .join("") +
        "    return imports;\n" +
        "  };\n" +
        "}\n";
      var augmentedGlue = preamble + jsGlue;
      return augmentedGlue;
  }

  /// Materialize the env-import table as live closures for the legacy raw
  /// WASM path: `new Function` receives the executor and cell id as
  /// parameters, so the generated expressions close over them exactly as a
  /// hand-written imports object would.
  function rawEnvImports(rawSelf, rawCellId) {
    var _rawExprs = _envImportExprs("rawSelf", "rawCellId");
    var _rawBody = Object.keys(_rawExprs)
      .map(function (k) { return k + ": " + _rawExprs[k]; })
      .join(", ");
    /* eslint-disable-next-line no-new-func */
    return new Function(
      "rawSelf",
      "rawCellId",
      "return {" + _rawBody + "};"
    )(rawSelf, rawCellId);
  }

  // ── Expose on the global scope ─────────────────────────────────────────────

  self.__IronpadExecutorGlue = {
    JSPI_UNSUPPORTED_MSG: JSPI_UNSUPPORTED_MSG,
    envImportExprs: _envImportExprs,
    rewriteBindgenGlue: rewriteBindgenGlue,
    rawEnvImports: rawEnvImports,
  };
})();
