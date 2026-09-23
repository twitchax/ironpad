//! Native browser dialogs: the one `window.confirm` gate every destructive
//! action goes through.

/// Prompt shared by the two local-notebook Delete buttons (home page card and
/// editor menu), so the question a user answers is the same from either.
#[cfg(feature = "hydrate")]
pub(crate) const DELETE_NOTEBOOK_CONFIRM: &str = "Delete this notebook? This cannot be undone.";

/// Ask the user to confirm `message` with the browser's native dialog.
///
/// `false` whenever the prompt cannot be shown (no window, or the call
/// throws), so a dialog that never appeared can never read as consent.
#[cfg(feature = "hydrate")]
pub(crate) fn confirm(message: &str) -> bool {
    web_sys::window().is_some_and(|w| w.confirm_with_message(message).unwrap_or(false))
}
