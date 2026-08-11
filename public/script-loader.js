// Route-aware loader for the shell's classic scripts.
//
// These used to be twelve unconditional tags in <head>. v0.19.5 deferred them
// so they stopped blocking first paint; this loads only the ones a route
// actually uses. The home page is a list of notebook cards and needs none of
// KaTeX, Prism, Monaco or sortable, which is 158KB of the 164KB.
//
// Inlined into the shell (include_str!), so it runs during head parse with no
// round trip of its own and can start fetching before the parser continues.

(function () {
  "use strict";

  var VERSION = window.__ironpadVersion || "";
  var loaded = {};

  // Each entry is the ordered list of files that define it.
  var SCRIPTS = {
    katex: ["/katex/katex.min.js", "/katex/render-math.js"],
    prism: ["/prism/prism.js", "/prism/highlight-code.js"],
    monaco: [
      "/monaco/vs/loader.js",
      "/monaco/init.js",
      "/monaco/languages.js",
      "/monaco/bridge.js",
    ],
    executor: ["/executor-bridge.js"],
    storage: ["/storage.js"],
    embed: ["/embed-frame.js"],
    sortable: ["/sortable.min.js"],
  };

  // KaTeX and Prism ship UMD bundles that check for AMD's `define` and, when
  // they find it, register as anonymous modules and never assign their globals.
  // Monaco's loader defines it. Ordering the tags used to be what kept them
  // apart, which stops being true the moment loading is route-dependent, so
  // these hide `define` for the duration of their own load instead. That makes
  // the two independent of each other rather than merely ordered.
  var UMD = { katex: true, prism: true };

  /** Load one file, preserving execution order against other pending loads. */
  function loadFile(src) {
    return new Promise(function (resolve, reject) {
      var s = document.createElement("script");
      // A dynamically inserted script is async by default; `async = false`
      // opts back into "ordered async": still non-blocking, but executed in
      // insertion order, which is what multi-file entries above rely on.
      s.async = false;
      s.src = VERSION ? src + "?v=" + VERSION : src;
      s.onload = function () {
        resolve();
      };
      s.onerror = function () {
        reject(new Error("failed to load " + src));
      };
      document.head.appendChild(s);
    });
  }

  /** Load every file of one entry, keeping AMD away from the UMD bundles. */
  function loadEntry(name) {
    var files = SCRIPTS[name];
    if (!files) return Promise.resolve();

    // Hide the AMD marker, not `define` itself. A UMD bundle takes the AMD
    // branch on `typeof define === "function" && define.amd`, so clearing the
    // marker sends it to the global branch instead. Removing `define` outright
    // would also break any Monaco module that happened to execute in this
    // window, and Monaco loads its own modules lazily and continuously.
    var amd = null;
    if (UMD[name] && typeof window.define === "function" && window.define.amd) {
      amd = window.define.amd;
      window.define.amd = undefined;
    }
    function restore() {
      if (amd !== null) window.define.amd = amd;
    }

    // Inserted together rather than chained, so the files download in
    // parallel; `async = false` is what orders their execution.
    return Promise.all(files.map(loadFile)).then(restore, function (err) {
      restore();
      throw err;
    });
  }

  /**
   * Ensure every named entry has executed. Memoised per name, so repeated
   * calls across route changes cost nothing.
   */
  function ensure(names) {
    // UMD entries go first. In a fresh batch that puts them ahead of Monaco's
    // loader, so `define` does not exist yet when they run and the masking
    // above is never needed. The masking covers the other case: a route
    // reached by client-side navigation, where Monaco loaded long ago.
    var ordered = names
      .filter(function (n) {
        return UMD[n];
      })
      .concat(
        names.filter(function (n) {
          return !UMD[n];
        }),
      );

    return Promise.all(
      ordered.map(function (n) {
        if (!loaded[n]) loaded[n] = loadEntry(n);
        return loaded[n];
      }),
    );
  }

  /**
   * Entries every route loads.
   *
   * These back globals that Rust reaches for synchronously from mount effects
   * (`IronpadMonaco` in the editor, `Sortable` in the cell list, and
   * `IronpadExecutor` wherever a cell runs). A late arrival would leave those
   * effects looking at an undefined global with no retry, so they are not
   * candidates for lazy loading until those call sites can await. Together
   * they are ~59KB against KaTeX and Prism's ~117KB.
   */
  var ALWAYS = ["storage", "monaco", "sortable", "executor"];

  /**
   * What a path needs on top of {@link ALWAYS}.
   *
   * KaTeX and Prism are safe to load per route precisely because neither has a
   * synchronous Rust caller: both install a MutationObserver and sweep the
   * existing DOM when they run, so arriving after the markup does is normal
   * operation rather than a race. Keep in sync with the routes in
   * `ironpad_app::App`.
   */
  function needsFor(path) {
    var markup = ["katex", "prism"];
    if (path === "/") return ALWAYS.slice();
    if (path.indexOf("/embed/") === 0) {
      return ALWAYS.concat(markup, ["embed"]);
    }
    if (
      path.indexOf("/local/") === 0 ||
      path.indexOf("/public/") === 0 ||
      path.indexOf("/shared/") === 0 ||
      path.indexOf("/mutable/") === 0 ||
      path.indexOf("/notebook/") === 0
    ) {
      return ALWAYS.concat(markup);
    }
    // An unknown path gets everything: loading too much is a slower page,
    // loading too little is a broken one.
    return Object.keys(SCRIPTS);
  }

  window.IronpadLoad = {
    ensure: ensure,
    needsFor: needsFor,
    /** Load whatever the current path needs. Called on client-side nav. */
    ensureForPath: function (path) {
      return ensure(needsFor(path));
    },
  };

  // Hydration waits on this. It used to be DOMContentLoaded, which was the
  // right signal when every script sat in the document; now the scripts are
  // inserted rather than parsed, so the loader's own promise is what says the
  // globals this route needs are actually defined.
  window.__ironpadShellReady = ensure(needsFor(window.location.pathname));
})();
