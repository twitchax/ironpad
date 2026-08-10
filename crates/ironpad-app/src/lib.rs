// The cell card is a plain <div> rather than a component. Components act as
// type-erasure boundaries, so inlining it collapsed the cell's view into a
// single deep tachys type and tripped the default limit: "queries overflow
// the depth limit!" when computing the hydrate_async layout.
#![recursion_limit = "512"]

#[cfg(feature = "ssr")]
pub mod auth;
#[cfg(feature = "ssr")]
pub mod compiler;
#[cfg(feature = "ssr")]
pub mod db;

/// Toolchain for normal and SIMD cell builds — the common case, tracked at a
/// recent nightly. The finicky feature cells keep their own pins so this one can
/// stay fresh without dragging them along: `AUTODIFF_TOOLCHAIN` and
/// `ATOMICS_TOOLCHAIN` in `compiler/build.rs`, selected by `cell_toolchain`.
///
/// Pinning per-invocation (rather than the host default) is the point: dev
/// hosts, CI, and the deploy image previously compiled plain cells on whatever
/// their DEFAULT toolchain was (nightly on dev, stable in the image), so
/// nightly-only code validated green locally and failed on prod.
///
/// **Pinned** rather than floating for the PRD-0041 reason: rolling nightlies
/// drift into breakage. This one is verified to build a wasm-bindgen +
/// `portable_simd` cell, and needs only the `wasm32-unknown-unknown` target and
/// `rust-src`. The heavier requirements live on the split-out pins: `enzyme` on
/// `AUTODIFF_TOOLCHAIN` (July 2026 nightlies ICE on autodiff typetrees for
/// slices, so autodiff stays back on 2026-06-01), and the atomics sysroot on
/// `ATOMICS_TOOLCHAIN`. Keep `docker/Dockerfile` and the CI workflow in sync
/// when bumping this.
///
/// Ungated (not `ssr`-only) because the client footer displays it too — one
/// source of truth for the toolchain most cells compile on.
pub const CELL_TOOLCHAIN: &str = "nightly-2026-07-14";

#[cfg(feature = "hydrate")]
pub(crate) mod blob_cache;
pub mod components;
pub(crate) mod model;
pub(crate) mod session;

pub mod pages;
pub(crate) mod sanitize;
pub mod server_fns;
pub mod storage;

use components::app_layout::AppLayout;
use components::toaster::{ToastHost, Toaster};
use leptos::prelude::*;
use leptos_meta::{provide_meta_context, HashedStylesheet, MetaTags, Title};
use leptos_router::{
    components::{Redirect, Route, Router, Routes},
    hooks::use_params_map,
    ParamSegment, SsrMode, StaticSegment,
};
use pages::{
    EmbedMutablePage, EmbedPublicPage, EmbedSharedPage, HomePage, MutableNotebookPage,
    NotebookEditorPage, PublicNotebookPage, SharedNotebookPage,
};

/// Appends a release-version query to a URL-stable static asset path so
/// browsers refetch it after every deploy. These scripts change with releases
/// but keep the same URL; without a cache-buster a heuristically-cached copy
/// can outlive several releases (the pkg bundle itself is content-hashed via
/// cargo-leptos `hash-files` instead).
fn versioned(path: &str) -> String {
    format!("{path}?v={}", env!("CARGO_PKG_VERSION"))
}

/// Server-side shell rendered around the app.
pub fn shell(options: LeptosOptions) -> impl IntoView {
    view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                // Inline theme init to prevent FOUC.
                <script>
                    "(function(){var t=localStorage.getItem('ironpad-theme');if(t==='light'){document.documentElement.setAttribute('data-theme','light');}}());"
                </script>

                // Captures DOMContentLoaded for `hydrate_body_when_shell_ready`.
                // It has to be recorded here, inline and early, because the
                // event is not queryable after the fact: `readyState` turns
                // "interactive" BEFORE deferred scripts run and stays that way
                // after DOMContentLoaded fires, so it cannot tell "shell
                // pending" from "shell ready". A promise settled by the event
                // itself is unambiguous whenever hydration gets around to
                // awaiting it.
                <script>
                    "window.__ironpadShellReady=new Promise(function(r){document.addEventListener('DOMContentLoaded',function(){r();},{once:true});});"
                </script>

                <meta charset="utf-8"/>
                <meta name="viewport" content="width=device-width, initial-scale=1"/>
                <link rel="icon" type="image/svg+xml" href="/favicon.svg"/>
                <link rel="stylesheet" href=versioned("/katex/katex.min.css")/>
                // App stylesheet: content-hashed filename (cargo-leptos hash-files),
                // rendered here because LeptosOptions is server-only.
                <HashedStylesheet options=options.clone() id="leptos"/>
                <AutoReload options=options.clone()/>
                <HydrationScripts options/>
                <MetaTags/>

                // Every script below is `defer`red. Without it the parser stops
                // at each one, and measured against production that cost 223ms
                // of a 378ms first contentful paint — 164KB of libraries the
                // home page never touches, fetched before anything is drawn.
                //
                // `defer` is the only correct attribute here: deferred classic
                // scripts still execute in DOCUMENT ORDER, which the two
                // orderings below depend on. `async` would not, and would let
                // Monaco's AMD loader win the race described next.
                //
                // The pairing invariant is in `hydrate_body_when_shell_ready`:
                // deferred scripts are guaranteed to run before
                // DOMContentLoaded, so hydration waits for that event rather
                // than reaching for a global whose script has not run.

                // KaTeX math rendering — before Monaco, whose AMD `define`
                // would otherwise capture this UMD bundle as a module and leave
                // `window.IronpadKaTeX` undefined.
                <script defer src=versioned("/katex/katex.min.js")></script>
                <script defer src=versioned("/katex/render-math.js")></script>

                // Prism syntax highlighting for rendered markdown code blocks —
                // also before Monaco's loader, for the same AMD `define` reason.
                <script defer src=versioned("/prism/prism.js")></script>
                <script defer src=versioned("/prism/highlight-code.js")></script>

                // Monaco editor: AMD loader + worker configuration + languages + Rust bridge.
                <script defer src=versioned("/monaco/vs/loader.js")></script>
                <script defer src=versioned("/monaco/init.js")></script>
                <script defer src=versioned("/monaco/languages.js")></script>
                <script defer src=versioned("/monaco/bridge.js")></script>

                // WASM cell executor (Web Worker bridge — delegates to executor-worker.js).
                <script defer src=versioned("/executor-bridge.js")></script>

                // IndexedDB notebook storage.
                <script defer src=versioned("/storage.js")></script>

                // Embed height reporter (no-op unless framed with ?embed_id=).
                <script defer src=versioned("/embed-frame.js")></script>

                // Drag-and-drop sortable library.
                <script defer src=versioned("/sortable.min.js")></script>
            </head>
            <body>
                <App/>
            </body>
        </html>
    }
}

/// Hydrates [`App`], but not before the shell's deferred scripts have run.
///
/// Deferring those scripts (see [`shell`]) is what keeps 164KB of libraries out
/// of the critical path, and it makes their execution concurrent with this
/// module rather than strictly before it. The orders can genuinely invert:
/// `/pkg/` is served `immutable` while the shell scripts are served
/// `no-cache`, so on a repeat visit the wasm is ready from cache with no
/// network at all while `storage.js` is still revalidating.
///
/// Two things then combine badly. `leptos::mount::hydrate_body` mounts
/// synchronously with no readiness handling of its own, and wasm-bindgen's
/// generated glue resolves a `js_namespace` at CALL time (`IronpadStorage.foo()`
/// as a bare global reference). So hydrating early does not fail at load, it
/// fails later and less legibly, the first time an effect reaches for a global
/// whose script has not run.
///
/// Deferred scripts are guaranteed to execute before `DOMContentLoaded`, which
/// makes that event exactly the signal needed. If the shell's capturing promise
/// is missing, this mounts immediately rather than never.
#[cfg(feature = "hydrate")]
pub fn hydrate_body_when_shell_ready() {
    use wasm_bindgen::JsCast;

    let ready = web_sys::window()
        .and_then(|w| {
            js_sys::Reflect::get(&w, &wasm_bindgen::JsValue::from_str("__ironpadShellReady")).ok()
        })
        .and_then(|v| v.dyn_into::<js_sys::Promise>().ok());

    let Some(ready) = ready else {
        leptos::mount::hydrate_body(App);
        return;
    };

    wasm_bindgen_futures::spawn_local(async move {
        // A rejected promise is not a reason to leave the page dead; the
        // shell only ever resolves this one.
        let _ = wasm_bindgen_futures::JsFuture::from(ready).await;
        leptos::mount::hydrate_body(App);
    });
}

/// Root application component.
///
/// Sets up leptos_meta context, provides the app-level [`Toaster`], and
/// defines routes for the home page and notebook editor. All routes are
/// wrapped in `AppLayout` which provides the header, content area, and
/// status bar; theming is ironpad's own (CSS custom properties plus the
/// `data-theme` attribute).
#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();
    provide_context(Toaster::new());

    view! {
        <Title text="ironpad"/>

        <Router>
            <AppLayout>
                <Routes fallback=|| "Page not found.".into_view()>
                    <Route path=StaticSegment("") view=HomePage/>
                    // Canonical scheme (PRD-0048): the prefix names the
                    // storage class — /local (private, IndexedDB), /public
                    // (bundled showcase, extension-less), /shared
                    // (content-addressed shares).
                    //
                    // The three server-backed notebook routes render with
                    // `SsrMode::Async` (PRD-0050). Their `<title>` and `og:`
                    // tags come from a Resource, and under the default
                    // out-of-order streaming the `<head>` is flushed before
                    // that Resource resolves — leptos_meta then patches the
                    // tags in with a script, which every link unfurler misses
                    // because none of them run JavaScript. Awaiting the
                    // notebook before the first byte is what puts the metadata
                    // in the document a crawler actually reads. All three load
                    // from local disk, so the added latency is a file read.
                    //
                    // /local is deliberately left streaming: it loads from
                    // IndexedDB in the browser, so there is nothing for the
                    // server to await and nothing for a crawler to see.
                    <Route path=(StaticSegment("local"), ParamSegment("id")) view=NotebookEditorPage/>
                    <Route path=(StaticSegment("public"), ParamSegment("filename")) view=PublicNotebookPage ssr=SsrMode::Async/>
                    <Route path=(StaticSegment("shared"), ParamSegment("hash")) view=SharedNotebookPage ssr=SsrMode::Async/>
                    // Mutable shares (PRD-0049): server-backed, author-updatable.
                    <Route path=(StaticSegment("mutable"), ParamSegment("id")) view=MutableNotebookPage ssr=SsrMode::Async/>
                    <Route path=(StaticSegment("embed"), StaticSegment("shared"), ParamSegment("hash")) view=EmbedSharedPage/>
                    <Route path=(StaticSegment("embed"), StaticSegment("public"), ParamSegment("filename")) view=EmbedPublicPage/>
                    <Route path=(StaticSegment("embed"), StaticSegment("mutable"), ParamSegment("id")) view=EmbedMutablePage/>
                    // Legacy routes redirect to canonical forever: bookmarks
                    // and old links must never break.
                    <Route path=(StaticSegment("notebook"), StaticSegment("public"), ParamSegment("filename")) view=LegacyPublicRedirect/>
                    <Route path=(StaticSegment("notebook"), ParamSegment("id")) view=LegacyLocalRedirect/>
                </Routes>
            </AppLayout>
        </Router>
        <ToastHost/>
    }
}

// ── Legacy route redirects (PRD-0048) ───────────────────────────────────────

/// `/notebook/{id}` → `/local/{id}`. Kept forever: private-notebook links
/// only live in the owner's own browser (bookmarks, history), but they
/// should keep working.
#[component]
fn LegacyLocalRedirect() -> impl IntoView {
    let id = use_params_map()
        .read_untracked()
        .get("id")
        .unwrap_or_default();
    view! { <Redirect path=format!("/local/{id}")/> }
}

/// `/notebook/public/{filename}` → `/public/{name}`, stripping the
/// `.ironpad` extension the legacy form carried.
#[component]
fn LegacyPublicRedirect() -> impl IntoView {
    let filename = use_params_map()
        .read_untracked()
        .get("filename")
        .unwrap_or_default();
    let name = filename
        .strip_suffix(".ironpad")
        .unwrap_or(&filename)
        .to_string();
    view! { <Redirect path=format!("/public/{name}")/> }
}

#[cfg(test)]
mod shell_asset_tests {
    use super::versioned;

    #[test]
    fn versioned_appends_the_release_version_query() {
        assert_eq!(
            versioned("/monaco/bridge.js"),
            format!("/monaco/bridge.js?v={}", env!("CARGO_PKG_VERSION"))
        );
    }
}
