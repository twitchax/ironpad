//! A collapsed-by-default section below a notebook's cell list.
//!
//! The cells are the story and these are the footnotes: the viewer's shared
//! source and Cargo.toml appendix, and the editor's shared editors and
//! metadata form all render as this one header-plus-body markup.

use leptos::prelude::*;

use crate::components::icon::{Chevron, IconData, IconLabel};

/// A header button (chevron, icon, label) over a body that starts collapsed.
///
/// The body is built only while expanded, so whatever it holds (a Monaco
/// editor, a form seeded from current state) mounts lazily on first expand
/// and re-seeds each time it opens.
#[component]
pub fn CollapsibleSection(
    icon: IconData,
    label: &'static str,
    children: ChildrenFn,
) -> impl IntoView {
    let collapsed = RwSignal::new(true);

    view! {
        <div class="view-only-shared-section">
            <button
                class="view-only-shared-header"
                on:click=move |_| collapsed.update(|c| *c = !*c)
            >
                <span class="ironpad-output-toggle"><Chevron expanded=Signal::derive(move || !collapsed.get())/></span>
                <IconLabel icon=icon label=label/>
            </button>
            {move || (!collapsed.get()).then(|| view! {
                <div class="view-only-shared-body">{children()}</div>
            })}
        </div>
    }
}
