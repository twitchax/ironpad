#[cfg(feature = "ssr")]
pub mod compiler;

#[cfg(feature = "ssr")]
pub mod notebook;

pub mod server_fns;

use leptos::prelude::*;
use leptos_meta::{provide_meta_context, MetaTags, Stylesheet, Title};
use leptos_router::{
    components::{Route, Router, Routes},
    ParamSegment, StaticSegment,
};
use thaw::{ConfigProvider, Theme};

/// Server-side shell rendered around the app.
pub fn shell(options: LeptosOptions) -> impl IntoView {
    view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8"/>
                <meta name="viewport" content="width=device-width, initial-scale=1"/>
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

/// Root application component.
///
/// Wraps the entire app in Thaw's ConfigProvider with a dark theme,
/// sets up leptos_meta context, and defines routes for the home page
/// and notebook editor.
#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();

    let theme = RwSignal::new(Theme::dark());

    view! {
        <Stylesheet id="leptos" href="/pkg/ironpad.css"/>
        <Title text="ironpad"/>

        <ConfigProvider theme>
            <Router>
                <main>
                    <Routes fallback=|| "Page not found.".into_view()>
                        <Route path=StaticSegment("") view=HomePage/>
                        <Route path=(StaticSegment("notebook"), ParamSegment("id")) view=NotebookEditorPage/>
                    </Routes>
                </main>
            </Router>
        </ConfigProvider>
    }
}

/// Placeholder home page — will be replaced by T-020.
#[component]
fn HomePage() -> impl IntoView {
    view! {
        <h1>"ironpad"</h1>
        <p>"Interactive Rust Notebooks"</p>
    }
}

/// Placeholder notebook editor page — will be replaced by T-021.
#[component]
fn NotebookEditorPage() -> impl IntoView {
    view! {
        <h1>"Notebook Editor"</h1>
        <p>"Loading notebook..."</p>
    }
}
