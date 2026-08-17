// The cell card is a plain <div> rather than a component. Components act as
// type-erasure boundaries, so inlining it collapsed the cell's view into a
// single deep tachys type and tripped the default limit: "queries overflow
// the depth limit!" when computing the hydrate_async layout.
#![recursion_limit = "512"]
// `--features ssr` ALONE is a diagnostic configuration, not a shipping one.
// Both things that build this crate for real unify `hydrate` in beside `ssr`:
// the workspace gate (`cargo clippy --all-targets`) because `ironpad-frontend`
// depends on `ironpad-app/hydrate`, and cargo-leptos because it builds both
// halves. Compiled ssr-alone, every hydrate-only import, binding, variant and
// method reads as dead, which is 32 diagnostics of pure noise for anyone
// iterating on this crate with `-p`.
//
// This costs no coverage, which is the only reason it is acceptable: the lints
// stay fully armed in the configuration that ships. An item that is genuinely
// unused is unused in the unified build too, and the gate is the unified build,
// so it still fails there. What is silenced here is exactly the set of items
// whose only consumer was compiled out by the flag that turned this attribute
// on. Scoped with `not(feature = "hydrate")` rather than applied outright for
// that reason: with `hydrate` present, none of this applies.
#![cfg_attr(
    not(feature = "hydrate"),
    allow(
        dead_code,
        unused_imports,
        unused_variables,
        unused_mut,
        clippy::needless_return,
        clippy::unused_self,
        clippy::unused_async,
        clippy::needless_pass_by_value
    )
)]

#[cfg(feature = "ssr")]
pub mod auth;
#[cfg(feature = "ssr")]
pub mod cache_tiers;
#[cfg(feature = "ssr")]
pub mod compiler;
#[cfg(feature = "ssr")]
pub mod db;

/// The toolchain every cell build uses, whatever its features (PRD-0067).
///
/// There used to be three nightly pins here, each held where some finicky
/// feature was known-good: autodiff on 2026-06-01, rayon/atomics on
/// 2025-12-22, everything else tracked fresh. Two of those three reasons had
/// expired without anyone re-testing them, and the deploy image was carrying
/// four nightlies (~2.9 GB) to honour them.
///
/// `nightly-2026-05-19` is the one date that satisfies all of it, and the
/// image already had it: the `BrowserPod` pack's `libexec/{rustc,cargo}` are
/// symlinks into this exact toolchain, so Linux cells (PRD-0066) pinned it
/// there regardless. Collapsing onto it took the image from four nightlies to
/// one.
///
/// **The ceiling is autodiff, and it is a hard one.** Nightlies from July 2026
/// ICE with `incorrect autodiff typetree handling for slice`, and because
/// `-Zautodiff=Enable` applies to the whole crate graph the ICE fires on a
/// transitive dependency (`icu_normalizer`), not on user code, so no cell-side
/// workaround exists. The floor is the `BrowserPod` pack, whose 22 prebuilt rlibs
/// are ABI-tied to this rustc. Between them sits one usable date.
///
/// **This value is load-bearing beyond cells**: it must equal
/// `BROWSERPOD_NIGHTLY` in `docker/browserpod.env`, or the image pays for a
/// second full toolchain. `browserpod_pin_matches_cell_toolchain` in
/// `compiler/build.rs` enforces that, so a `BrowserPod` bump surfaces the
/// decision instead of silently regrowing the image.
///
/// Pinning per-invocation (rather than the host default) is the point: dev
/// hosts, CI, and the deploy image previously compiled plain cells on whatever
/// their DEFAULT toolchain was (nightly on dev, stable in the image), so
/// nightly-only code validated green locally and failed on prod. Keep
/// `docker/Dockerfile` and the CI workflow in sync when bumping this; the
/// `toolchain_pins_are_installed_by_the_image` test checks that they are.
///
/// Ungated (not `ssr`-only) because the client footer displays it too — one
/// source of truth for the toolchain cells compile on.
pub const CELL_TOOLCHAIN: &str = "nightly-2026-05-19";

#[cfg(feature = "hydrate")]
pub(crate) mod blob_cache;
pub mod components;
pub(crate) mod model;
pub(crate) mod session;

pub mod pages;
pub(crate) mod sanitize;
pub mod server_fns;
pub mod storage;

#[cfg(feature = "hydrate")]
use wasm_bindgen::JsCast as _;

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
    AdminPage, EmbedMutablePage, EmbedPublicPage, EmbedSharedPage, HomePage, MutableNotebookPage,
    NotebookEditorPage, PublicNotebookPage, SharedNotebookPage,
};

/// Appends a release-version query to a URL-stable static asset path so
/// browsers refetch it after every deploy. These scripts change with releases
/// but keep the same URL; without a cache-buster a heuristically-cached copy
/// can outlive several releases (the pkg bundle itself is content-hashed via
/// cargo-leptos `hash-files` instead).
pub(crate) fn versioned(path: &str) -> String {
    format!("{path}?v={}", env!("CARGO_PKG_VERSION"))
}

/// Server-side shell rendered around the app.
pub fn shell(options: LeptosOptions) -> impl IntoView {
    view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                // `charset` must land inside the first 1024 bytes of the
                // document or the browser falls back to guessing the encoding,
                // so it leads. The inlined loader below is ~5KB and would push
                // it out of range from any later position.
                <meta charset="utf-8"/>
                <meta name="viewport" content="width=device-width, initial-scale=1"/>

                // Inline theme init to prevent FOUC.
                <script>
                    "(function(){var t=localStorage.getItem('ironpad-theme');if(t==='light'){document.documentElement.setAttribute('data-theme','light');}}());"
                </script>

                // Route-aware script loading (`public/script-loader.js`). The
                // shell used to load 164KB of libraries on every route; the
                // home page is a list of notebook cards and renders no
                // markdown, so it skips KaTeX and Prism and drops to 58KB.
                // Monaco, sortable and the executor still load everywhere
                // because Rust reaches for their globals synchronously from
                // mount effects; see the loader's ALWAYS.
                //
                // Inlined rather than fetched, for two reasons: it runs during
                // head parse and can start the route's scripts before the
                // parser continues, and an external file would not execute
                // until after DOMContentLoaded, by which point hydration has
                // already read the readiness promise it defines.
                //
                // The version is threaded through a global because the loader
                // is plain JS and cannot read `CARGO_PKG_VERSION`. It appends
                // the same `?v=` cache-buster `versioned` does, for the reason
                // given there.
                <script>
                    {format!("window.__ironpadVersion={:?};", env!("CARGO_PKG_VERSION"))}
                </script>
                <script>{include_str!("../../../public/script-loader.js")}</script>

                <link rel="icon" type="image/svg+xml" href="/favicon.svg"/>
                <link rel="stylesheet" href=versioned("/katex/katex.min.css")/>
                // App stylesheet: content-hashed filename (cargo-leptos hash-files),
                // rendered here because LeptosOptions is server-only.
                <HashedStylesheet options=options.clone() id="leptos"/>
                <AutoReload options=options.clone()/>
                <HydrationScripts options/>
                <MetaTags/>
            </head>
            <body>
                <App/>
            </body>
        </html>
    }
}

/// Hydrates [`App`], but not before the current route's scripts have run.
///
/// The shell loads scripts per route (see [`shell`]), which makes their
/// execution concurrent with this module rather than strictly before it. The
/// orders can genuinely invert: `/pkg/` is served `immutable` while the shell
/// scripts are served `no-cache`, so on a repeat visit the wasm is ready from
/// cache with no network at all while `storage.js` is still being fetched.
///
/// Two things then combine badly. `leptos::mount::hydrate_body` mounts
/// synchronously with no readiness handling of its own, and wasm-bindgen's
/// generated glue resolves a `js_namespace` at CALL time (`IronpadStorage.foo()`
/// as a bare global reference). So hydrating early does not fail at load, it
/// fails later and less legibly, the first time an effect reaches for a global
/// whose script has not run.
///
/// The loader publishes `__ironpadShellReady` for exactly this. It was
/// `DOMContentLoaded` when every script sat in the document as a deferred tag;
/// scripts are inserted now rather than parsed, so that event no longer implies
/// anything about them and the loader's own promise is the signal. If it is
/// missing, this mounts immediately rather than never.
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

/// Loads the scripts a route needs after a client-side navigation.
///
/// The shell only knows the FIRST path, and a client-side navigation never
/// re-runs it, so a route reached without a page load has to pull its own
/// scripts. Only KaTeX and Prism are route-dependent, and both sweep the
/// existing DOM when they run, so arriving after the markup is normal
/// operation rather than a race.
///
/// Must render INSIDE `<Router>`: `use_location` panics without that context,
/// and it does so at hydration, which takes the whole app down rather than
/// just this effect.
#[component]
fn RouteScripts() -> impl IntoView {
    #[cfg(feature = "hydrate")]
    {
        let location = leptos_router::hooks::use_location();
        Effect::new(move |_| {
            let path = location.pathname.get();
            let Some(window) = web_sys::window() else {
                return;
            };
            let Ok(loader) =
                js_sys::Reflect::get(&window, &wasm_bindgen::JsValue::from_str("IronpadLoad"))
            else {
                return;
            };
            let Ok(ensure) =
                js_sys::Reflect::get(&loader, &wasm_bindgen::JsValue::from_str("ensureForPath"))
            else {
                return;
            };
            if let Some(f) = ensure.dyn_ref::<js_sys::Function>() {
                let _ = f.call1(&loader, &wasm_bindgen::JsValue::from_str(&path));
            }
        });
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
            <RouteScripts/>
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
                    // Admin panel (PRD-0063). Rendered for everyone and
                    // gated by its server fns, which is what keeps instance
                    // state off the wire; a non-admin sees "Page not found."
                    //
                    // `SsrMode::Async` for the status code, not for metadata.
                    // Under the default out-of-order streaming the response
                    // status is committed when the shell flushes, which races
                    // the gate: an anonymous visitor is denied instantly (no
                    // cookie, no query) and got a 404, while a signed-in
                    // non-admin needed a session lookup first and got a 200
                    // with a not-found body. Awaiting the resource before the
                    // first byte makes the status honest for both.
                    <Route path=StaticSegment("admin") view=AdminPage ssr=SsrMode::Async/>
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
