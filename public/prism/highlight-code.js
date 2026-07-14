// Syntax-highlight all fenced code blocks inside a container (or the whole
// document). pulldown-cmark emits fenced code as:
//   <pre><code class="language-rust">…</code></pre>
// Prism reads that `language-*` class and rewrites the element's content into
// `<span class="token …">` spans, which style/main.scss colors per theme.
//
// Mirrors katex/render-math.js: a guarded highlight pass plus a MutationObserver
// so hydration, SPA navigation, and markdown-cell edits all get highlighted
// without any component wiring.

(function () {
  "use strict";

  function highlightIn(root) {
    if (typeof Prism === "undefined") return;
    var elems = root.querySelectorAll('code[class*="language-"]');
    for (var i = 0; i < elems.length; i++) {
      var el = elems[i];
      // A re-rendered markdown preview replaces the <code> node wholesale, so
      // the fresh element lacks this marker and gets re-highlighted; an
      // unchanged element is skipped.
      if (el.getAttribute("data-prism-highlighted")) continue;
      try {
        Prism.highlightElement(el);
        el.setAttribute("data-prism-highlighted", "1");
      } catch (_) {
        // Leave the raw code visible on error.
      }
    }
  }

  // Expose globally so Leptos components can call it after DOM updates.
  window.IronpadPrism = { highlightIn: highlightIn };

  // Observe the DOM for new code blocks (hydration / SPA nav / cell edits).
  var observer = new MutationObserver(function (mutations) {
    for (var i = 0; i < mutations.length; i++) {
      var added = mutations[i].addedNodes;
      for (var j = 0; j < added.length; j++) {
        var node = added[j];
        if (node.nodeType !== 1) continue;
        if (
          node.matches &&
          node.matches('code[class*="language-"]')
        ) {
          highlightIn(node.parentElement || document.body);
        } else if (node.querySelectorAll) {
          var inner = node.querySelectorAll('code[class*="language-"]');
          if (inner.length > 0) highlightIn(node);
        }
      }
    }
  });

  observer.observe(document.documentElement, {
    childList: true,
    subtree: true,
  });

  // Initial pass for SSR-rendered content.
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", function () {
      highlightIn(document.body);
    });
  } else {
    highlightIn(document.body);
  }
})();
