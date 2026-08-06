/**
 * ironpad embed loader.
 *
 * Turns lightweight markup on any third-party page into an auto-resizing
 * iframe hosting a read-only, runnable ironpad notebook. Two snippet styles:
 *
 *   1. Self-invoking script tag:
 *        <script src="https://ironpad.example/embed.js"
 *                data-notebook="shared/abc123def4567890" async></script>
 *
 *   2. Placeholder divs (one script, many embeds):
 *        <div class="ironpad-embed" data-notebook="public/mandelbrot.ironpad"></div>
 *        <script src="https://ironpad.example/embed.js" async></script>
 *
 * `data-notebook` is either "shared/{hash}" or "public/{filename}". The iframe
 * origin is derived from this script's own src, so the loader works on any
 * ironpad deployment without configuration.
 *
 * Height protocol: the embedded page posts
 *   { type: "ironpad:embed:height", id: <embed id>, height: <px> }
 * on load and on content resize; the loader validates the event origin and
 * matches ids so multiple embeds on one page can't cross-talk.
 */
(function () {
  "use strict";

  var script = document.currentScript;
  if (!script) return;

  var origin;
  try {
    origin = new URL(script.src).origin;
  } catch (_e) {
    return;
  }

  var seq = 0;

  /** Validate and normalize a data-notebook value; null if malformed. */
  function embedPath(spec) {
    if (typeof spec !== "string") return null;
    var m = /^(shared|public|mutable)\/([A-Za-z0-9._-]+)$/.exec(spec.trim());
    if (!m) return null;
    return "/embed/" + m[1] + "/" + encodeURIComponent(m[2]);
  }

  /** Replace (or fill) a target element with the notebook iframe. */
  function mount(target, spec, autorun) {
    var path = embedPath(spec);
    if (!path) return;

    var id = "ironpad-embed-" + Date.now().toString(36) + "-" + seq++;
    var frame = document.createElement("iframe");
    frame.src =
      origin +
      path +
      "?embed_id=" +
      encodeURIComponent(id) +
      // Embedder opt-in (data-autorun): honored only for public/first-party
      // notebooks — the shared embed route ignores it (PRD-0040).
      (autorun ? "&autorun=1" : "");
    frame.id = id;
    frame.loading = "lazy";
    frame.style.width = "100%";
    frame.style.border = "0";
    frame.style.height = "480px"; // placeholder until the first height message
    frame.setAttribute("title", "ironpad notebook");
    frame.setAttribute("allow", "clipboard-write");

    target.appendChild(frame);

    window.addEventListener("message", function (event) {
      if (event.origin !== origin) return;
      var msg = event.data;
      if (!msg || msg.type !== "ironpad:embed:height" || msg.id !== id) return;
      var h = Number(msg.height);
      if (Number.isFinite(h) && h > 0) {
        frame.style.height = Math.ceil(h) + "px";
      }
    });
  }

  // Style 2: placeholder divs anywhere on the page (mount each exactly once).
  var placeholders = document.querySelectorAll(
    ".ironpad-embed[data-notebook]:not([data-ironpad-mounted])"
  );
  for (var i = 0; i < placeholders.length; i++) {
    placeholders[i].setAttribute("data-ironpad-mounted", "");
    mount(
      placeholders[i],
      placeholders[i].getAttribute("data-notebook"),
      placeholders[i].hasAttribute("data-autorun")
    );
  }

  // Style 1: the script tag itself names a notebook — mount in its place.
  var own = script.getAttribute("data-notebook");
  if (own) {
    var holder = document.createElement("div");
    holder.className = "ironpad-embed";
    holder.setAttribute("data-ironpad-mounted", "");
    script.parentNode.insertBefore(holder, script);
    mount(holder, own, script.hasAttribute("data-autorun"));
  }
})();
