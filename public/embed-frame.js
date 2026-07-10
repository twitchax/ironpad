/**
 * ironpad embed frame reporter.
 *
 * Loaded only meaningfully by the /embed/* routes (no-op elsewhere). Posts the
 * layout's rendered height to the parent window whenever it changes so the
 * host page's embed.js can size the iframe (protocol documented in embed.js).
 *
 * Measurement note: `document.documentElement.scrollHeight` is clamped to the
 * viewport, so inside an iframe it can only ever report "at least the iframe
 * height" — the frame could grow but never shrink, and short notebooks would
 * read as exactly the placeholder size. The app layout root's `scrollHeight`
 * is viewport-independent, so it reflects the true content height both ways.
 */
(function () {
  "use strict";

  if (window.parent === window) return;

  var params = new URLSearchParams(window.location.search);
  var id = params.get("embed_id");
  if (!id) return;

  var lastHeight = 0;

  function contentHeight() {
    var root = document.querySelector(".ironpad-root-layout");
    var h = root ? root.scrollHeight : document.documentElement.scrollHeight;
    // Small buffer so rounding never clips the badge's bottom border.
    return h + 4;
  }

  function report() {
    var height = contentHeight();
    if (height === lastHeight) return;
    lastHeight = height;
    // The loader validates event.origin and the id; "*" here is safe because
    // the payload carries no secrets (a pixel height and a public embed id).
    window.parent.postMessage(
      { type: "ironpad:embed:height", id: id, height: height },
      "*"
    );
  }

  // Content mounts and grows asynchronously (Suspense, Monaco, cell outputs),
  // and none of it resizes a stable observable ancestor — so poll cheaply and
  // post only on change.
  setInterval(report, 400);
  window.addEventListener("load", report);
  report();
})();
