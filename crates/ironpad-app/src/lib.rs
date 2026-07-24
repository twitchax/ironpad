// The cell card is a plain <div> rather than a component. Components act as
// type-erasure boundaries, so inlining it collapsed the cell's view into a
// single deep tachys type and tripped the default limit: "queries overflow
// the depth limit!" when computing the hydrate_async layout.
#![recursion_limit = "512"]

#[cfg(feature = "ssr")]
pub mod compiler;

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
    ParamSegment, StaticSegment,
};
use pages::{
    EmbedPublicPage, EmbedSharedPage, HomePage, MutableNotebookPage, NotebookEditorPage,
    PublicNotebookPage, SharedNotebookPage,
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

                // KaTeX math rendering — loaded before Monaco (no defer) to avoid AMD `define` conflict.
                <script src=versioned("/katex/katex.min.js")></script>
                <script src=versioned("/katex/render-math.js")></script>

                // Prism syntax highlighting for rendered markdown code blocks —
                // also before Monaco's loader, for the same AMD `define` reason.
                <script src=versioned("/prism/prism.js")></script>
                <script src=versioned("/prism/highlight-code.js")></script>

                // Monaco editor: AMD loader + worker configuration + languages + Rust bridge.
                <script src=versioned("/monaco/vs/loader.js")></script>
                <script src=versioned("/monaco/init.js")></script>
                <script src=versioned("/monaco/languages.js")></script>
                <script src=versioned("/monaco/bridge.js")></script>

                // WASM cell executor (Web Worker bridge — delegates to executor-worker.js).
                <script src=versioned("/executor-bridge.js")></script>

                // IndexedDB notebook storage.
                <script src=versioned("/storage.js")></script>

                // Embed height reporter (no-op unless framed with ?embed_id=).
                <script src=versioned("/embed-frame.js")></script>

                // Drag-and-drop sortable library.
                <script src=versioned("/sortable.min.js")></script>
            </head>
            <body>
                <App/>
            </body>
        </html>
    }
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
                    <Route path=(StaticSegment("local"), ParamSegment("id")) view=NotebookEditorPage/>
                    <Route path=(StaticSegment("public"), ParamSegment("filename")) view=PublicNotebookPage/>
                    <Route path=(StaticSegment("shared"), ParamSegment("hash")) view=SharedNotebookPage/>
                    // Mutable shares (PRD-0049): server-backed, author-updatable.
                    <Route path=(StaticSegment("mutable"), ParamSegment("id")) view=MutableNotebookPage/>
                    <Route path=(StaticSegment("embed"), StaticSegment("shared"), ParamSegment("hash")) view=EmbedSharedPage/>
                    <Route path=(StaticSegment("embed"), StaticSegment("public"), ParamSegment("filename")) view=EmbedPublicPage/>
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
