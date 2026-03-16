// Render all KaTeX math elements inside a container (or the whole document).
// pulldown-cmark emits:
//   <span class="math math-inline">tex</span>
//   <span class="math math-display">tex</span>
// We replace the text content of each with KaTeX-rendered HTML.

(function () {
  "use strict";

  function renderMathIn(root) {
    if (typeof katex === "undefined") return;
    var elems = root.querySelectorAll("span.math");
    for (var i = 0; i < elems.length; i++) {
      var el = elems[i];
      if (el.getAttribute("data-katex-rendered")) continue;
      var displayMode = el.classList.contains("math-display");
      try {
        katex.render(el.textContent, el, {
          displayMode: displayMode,
          throwOnError: false,
        });
        el.setAttribute("data-katex-rendered", "1");
      } catch (_) {
        // Leave raw TeX visible on error.
      }
    }
  }

  // Expose globally so Leptos components can call it after DOM updates.
  window.IronpadKaTeX = { renderMathIn: renderMathIn };

  // Also observe the DOM for new math elements (covers hydration / SPA nav).
  var observer = new MutationObserver(function (mutations) {
    for (var i = 0; i < mutations.length; i++) {
      var added = mutations[i].addedNodes;
      for (var j = 0; j < added.length; j++) {
        var node = added[j];
        if (node.nodeType === 1) {
          if (node.classList && node.classList.contains("math")) {
            renderMathIn(node.parentElement || document.body);
          } else if (node.querySelectorAll) {
            var inner = node.querySelectorAll("span.math");
            if (inner.length > 0) renderMathIn(node);
          }
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
      renderMathIn(document.body);
    });
  } else {
    renderMathIn(document.body);
  }
})();
