#[cfg(feature = "ssr")]
pub mod compiler;

/// Pinned toolchain for ALL cell builds except rayon/atomics cells (which keep
/// their own pin — see `ATOMICS_TOOLCHAIN` in `compiler/build.rs`).
///
/// One toolchain everywhere is the point: dev hosts, CI, and the deploy image
/// previously compiled plain cells on whatever their DEFAULT toolchain was
/// (nightly on dev, stable in the image), so nightly-only code validated green
/// locally and failed on prod. Pinning per-invocation removes the divergence.
///
/// **Pinned** rather than floating for the PRD-0041 reason: rolling nightlies
/// drift into breakage (the 2026-07 nightly ICEs on autodiff typetrees for
/// slices). This nightly carries the `enzyme` rustup component (a matched
/// libEnzyme/LLVM pair for `std::autodiff` cells), `rust-src` (for
/// `-Zbuild-std` when autodiff+rayon combine), the wasm32 target, and
/// `portable_simd` for SIMD cells. The deploy image and dev hosts must install
/// it with those components; keep `docker/Dockerfile` in sync.
///
/// Ungated (not `ssr`-only) because the client footer displays it too — one
/// source of truth for what compiles cells.
pub const CELL_TOOLCHAIN: &str = "nightly-2026-06-01";

pub mod components;
pub(crate) mod model;
pub(crate) mod session;

pub mod pages;
pub(crate) mod sanitize;
pub mod server_fns;
pub mod storage;

use components::app_layout::AppLayout;
use leptos::prelude::*;
use leptos_meta::{provide_meta_context, MetaTags, Stylesheet, Title};
use leptos_router::{
    components::{Route, Router, Routes},
    ParamSegment, StaticSegment,
};
use pages::{
    EmbedPublicPage, EmbedSharedPage, HomePage, NotebookEditorPage, PublicNotebookPage,
    SharedNotebookPage,
};
use thaw::{ConfigProvider, Theme, ToastPosition, ToasterProvider};

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
                <link rel="stylesheet" href="/katex/katex.min.css"/>
                <AutoReload options=options.clone()/>
                <HydrationScripts options/>
                <MetaTags/>

                // KaTeX math rendering — loaded before Monaco (no defer) to avoid AMD `define` conflict.
                <script src="/katex/katex.min.js"></script>
                <script src="/katex/render-math.js"></script>

                // Monaco editor: AMD loader + worker configuration + languages + Rust bridge.
                <script src="/monaco/vs/loader.js"></script>
                <script src="/monaco/init.js"></script>
                <script src="/monaco/languages.js"></script>
                <script src="/monaco/bridge.js"></script>

                // WASM cell executor (Web Worker bridge — delegates to executor-worker.js).
                <script src="/executor-bridge.js"></script>

                // IndexedDB notebook storage.
                <script src="/storage.js"></script>

                // Embed height reporter (no-op unless framed with ?embed_id=).
                <script src="/embed-frame.js"></script>

                // Drag-and-drop sortable library.
                <script src="/sortable.min.js"></script>
            </head>
            <body>
                <App/>
            </body>
        </html>
    }
}

/// Root application component.
///
/// Wraps the entire app in Thaw's ConfigProvider with a dark theme,
/// sets up leptos_meta context, and defines routes for the home page
/// and notebook editor. All routes are wrapped in `AppLayout` which
/// provides the header, content area, and status bar.
#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();

    let theme = RwSignal::new(Theme::dark());

    view! {
        <Stylesheet id="leptos" href="/pkg/ironpad.css"/>
        <Title text="ironpad"/>

        <ConfigProvider theme>
            <ToasterProvider position=ToastPosition::BottomEnd>
                <Router>
                    <AppLayout>
                        <Routes fallback=|| "Page not found.".into_view()>
                            <Route path=StaticSegment("") view=HomePage/>
                            <Route path=(StaticSegment("notebook"), StaticSegment("public"), ParamSegment("filename")) view=PublicNotebookPage/>
                            <Route path=(StaticSegment("shared"), ParamSegment("hash")) view=SharedNotebookPage/>
                            <Route path=(StaticSegment("embed"), StaticSegment("shared"), ParamSegment("hash")) view=EmbedSharedPage/>
                            <Route path=(StaticSegment("embed"), StaticSegment("public"), ParamSegment("filename")) view=EmbedPublicPage/>
                            <Route path=(StaticSegment("notebook"), ParamSegment("id")) view=NotebookEditorPage/>
                        </Routes>
                    </AppLayout>
                </Router>
            </ToasterProvider>
        </ConfigProvider>
    }
}
