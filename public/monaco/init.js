// Monaco editor initialization for ironpad.
// Configures the AMD loader and worker paths so Monaco runs fully self-hosted.

(function () {
  // Tell the AMD loader where to find Monaco modules.
  require.config({
    paths: { vs: "/monaco/vs" },
    // Suppress harmless "Duplicate definition" warnings from the Monaco AMD
    // bundle — the bundled editor.main.js can trigger a re-define during
    // dependency resolution.
    ignoreDuplicateModules: ["vs/editor/editor.main"],
  });

  // Configure Monaco environment for worker URLs.
  // Workers are served from /monaco/vs/assets/ with content-hashed filenames.
  window.MonacoEnvironment = {
    getWorkerUrl: function (_moduleId, label) {
      var base = "/monaco/vs/assets/";

      if (label === "json") return base + "json.worker-DKiEKt88.js";
      if (label === "css" || label === "scss" || label === "less")
        return base + "css.worker-HnVq6Ewq.js";
      if (label === "html" || label === "handlebars" || label === "razor")
        return base + "html.worker-B51mlPHg.js";
      if (label === "typescript" || label === "javascript")
        return base + "ts.worker-CMbG-7ft.js";

      // Default editor worker (handles diff, tokenization, etc.).
      return base + "editor.worker-Be8ye1pW.js";
    },
  };
})();
