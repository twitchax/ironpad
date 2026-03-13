#[cfg(feature = "ssr")]
pub mod compiler;

pub mod components;
pub(crate) mod model;

pub mod pages;
pub mod server_fns;
pub mod storage;

use components::app_layout::AppLayout;
use leptos::prelude::*;
use leptos_meta::{provide_meta_context, MetaTags, Stylesheet, Title};
use leptos_router::{
    components::{Route, Router, Routes},
    ParamSegment, StaticSegment,
};
use pages::{HomePage, NotebookEditorPage, PublicNotebookPage, SharedNotebookPage};
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
                <AutoReload options=options.clone()/>
                <HydrationScripts options/>
                <MetaTags/>

                // Monaco editor: AMD loader + worker configuration + languages + Rust bridge.
                <script src="/monaco/vs/loader.js"></script>
                <script src="/monaco/init.js"></script>
                <script src="/monaco/languages.js"></script>
                <script src="/monaco/bridge.js"></script>

                // WASM cell executor.
                <script src="/executor.js"></script>

                // IndexedDB notebook storage.
                <script src="/storage.js"></script>

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
                            <Route path=(StaticSegment("notebook"), ParamSegment("id")) view=NotebookEditorPage/>
                        </Routes>
                    </AppLayout>
                </Router>
            </ToasterProvider>
        </ConfigProvider>
    }
}
