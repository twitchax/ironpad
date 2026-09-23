//! Full-panel notices the pages show in place of a notebook: the loading
//! state and the error boundary.
//!
//! Every notebook route and embed renders the same two markups, so each lives
//! here once. Status codes are NOT decided here: a caller that owes a 404
//! still calls `mark_not_found()` beside the notice, since the status is a
//! property of the route's answer and not of the panel that describes it.

use leptos::prelude::*;

use crate::components::icon::{Icon, IconData};

/// The error boundary: an icon, a message, and an optional hint line.
///
/// `children`, when given, is the hint's content (wrapped in the hint
/// paragraph here), so a caller can put a link in it.
#[component]
pub(crate) fn ErrorNotice(
    icon: IconData,
    #[prop(into)] message: String,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    view! {
        <div class="ironpad-error-boundary">
            <div class="ironpad-error-boundary-icon"><Icon icon=icon/></div>
            <p class="ironpad-error-boundary-message">{message}</p>
            {children.map(|hint| view! {
                <p class="ironpad-error-boundary-hint">{hint()}</p>
            })}
        </div>
    }
}

/// The loading state a page shows while its notebook resolves.
#[component]
pub(crate) fn LoadingNotice(#[prop(into)] message: String) -> impl IntoView {
    view! {
        <div class="ironpad-loading">
            <p>{message}</p>
        </div>
    }
}
