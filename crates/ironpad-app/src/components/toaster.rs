//! App-level toast notifications.
//!
//! A minimal, owned replacement for Thaw's toaster: a [`Toaster`] handle is
//! provided as context at the app root, and [`ToastHost`] renders the live
//! toasts as a fixed overlay (bottom-right). Every dispatch site passes plain
//! strings, so the API is string-based rather than view-based.

use leptos::prelude::*;

/// Visual intent of a toast; colors the title.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "hydrate", derive(serde::Serialize, serde::Deserialize))]
pub enum ToastIntent {
    Success,
    Error,
    /// Neutral progress notice ("Pushing…"): something started, and nothing
    /// has succeeded or failed yet.
    Info,
}

/// `sessionStorage` key for a toast queued to survive a full page reload.
#[cfg(feature = "hydrate")]
const PENDING_TOAST_KEY: &str = "ironpad-pending-toast";

/// A toast serialized across a reload (Pull Latest ends in one): the live
/// toast list dies with the page, so the message rides `sessionStorage`.
#[cfg(feature = "hydrate")]
#[derive(serde::Serialize, serde::Deserialize)]
struct PendingToast {
    intent: ToastIntent,
    title: String,
    body: String,
    timeout_secs: u32,
}

#[cfg(feature = "hydrate")]
fn session_storage() -> Option<web_sys::Storage> {
    web_sys::window().and_then(|w| w.session_storage().ok().flatten())
}

/// One live toast.
#[derive(Clone)]
struct ToastEntry {
    id: u64,
    title: String,
    body: String,
    intent: ToastIntent,
}

/// Handle for dispatching toasts.
///
/// `Copy`, so it can be moved into event handlers and async continuations that
/// outlive the dispatching component — the underlying signal is owned by the
/// app root and lives for the page lifetime, which is what makes dispatching
/// from post-await continuations disposal-safe.
#[derive(Clone, Copy)]
pub struct Toaster {
    toasts: RwSignal<Vec<ToastEntry>>,
}

impl Toaster {
    pub(crate) fn new() -> Self {
        Self {
            toasts: RwSignal::new(Vec::new()),
        }
    }

    /// Resolves the app-level toaster from context.
    ///
    /// Must be called inside a reactive owner (component setup); callbacks
    /// that run outside any owner (JS event closures, `FileReader` callbacks)
    /// must have the handle resolved by their creator and moved in.
    pub fn expect_context() -> Self {
        expect_context::<Self>()
    }

    /// Shows a toast for `timeout_secs` seconds.
    pub fn toast(
        &self,
        intent: ToastIntent,
        title: impl Into<String>,
        body: impl Into<String>,
        timeout_secs: u32,
    ) {
        let title = title.into();
        let body = body.into();
        let mut new_id = 0;
        self.toasts.update(|list| {
            new_id = list.iter().map(|t| t.id).max().map_or(0, |m| m + 1);
            list.push(ToastEntry {
                id: new_id,
                title,
                body,
                intent,
            });
        });

        // Auto-dismiss. Browser-only: on the server a render is one-shot, so
        // there is nothing to dismiss. The closure captures only the app-root
        // signal (never a page-scoped one), so a late fire cannot hit a
        // disposed value; `once_into_js` frees the closure after it runs.
        #[cfg(all(feature = "hydrate", target_arch = "wasm32"))]
        {
            use wasm_bindgen::prelude::*;

            let toasts = self.toasts;
            let dismiss = Closure::once_into_js(move || {
                let _ = toasts.try_update(|list| list.retain(|t| t.id != new_id));
            });
            if let Some(window) = web_sys::window() {
                #[allow(clippy::cast_possible_wrap)]
                let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(
                    dismiss.unchecked_ref(),
                    (timeout_secs * 1000) as i32,
                );
            }
        }
        #[cfg(not(all(feature = "hydrate", target_arch = "wasm32")))]
        let _ = timeout_secs;
    }

    /// Queues a toast to show after the next full page load, for flows that
    /// end in `location.reload()` (Pull Latest): the live toast list dies
    /// with the page, so the message rides `sessionStorage` and [`ToastHost`]
    /// drains it on the next hydrate. Associated rather than a method — the
    /// toast targets the *next* page's toaster, not this one.
    #[cfg(feature = "hydrate")]
    pub fn toast_after_reload(
        intent: ToastIntent,
        title: impl Into<String>,
        body: impl Into<String>,
        timeout_secs: u32,
    ) {
        let pending = PendingToast {
            intent,
            title: title.into(),
            body: body.into(),
            timeout_secs,
        };
        let Some(storage) = session_storage() else {
            return;
        };
        if let Ok(json) = serde_json::to_string(&pending) {
            let _ = storage.set_item(PENDING_TOAST_KEY, &json);
        }
    }

    /// Shows (and clears) a queued after-reload toast, if one exists.
    #[cfg(feature = "hydrate")]
    fn drain_pending(self) {
        let Some(storage) = session_storage() else {
            return;
        };
        let Ok(Some(json)) = storage.get_item(PENDING_TOAST_KEY) else {
            return;
        };
        let _ = storage.remove_item(PENDING_TOAST_KEY);
        if let Ok(p) = serde_json::from_str::<PendingToast>(&json) {
            self.toast(p.intent, p.title, p.body, p.timeout_secs);
        }
    }
}

/// Fixed overlay rendering the live toasts; mounted once at the app root.
#[component]
pub fn ToastHost() -> impl IntoView {
    let toaster = Toaster::expect_context();

    // Show any toast queued across a reload (Pull Latest ends in one).
    #[cfg(feature = "hydrate")]
    Effect::new(move || toaster.drain_pending());

    view! {
        <div class="ironpad-toaster">
            <For
                each=move || toaster.toasts.get()
                key=|t| t.id
                children=|t: ToastEntry| {
                    let title_class = match t.intent {
                        ToastIntent::Success => {
                            "ironpad-toast-title ironpad-toast-title--success"
                        }
                        ToastIntent::Error => "ironpad-toast-title ironpad-toast-title--error",
                        ToastIntent::Info => "ironpad-toast-title ironpad-toast-title--info",
                    };
                    view! {
                        <div class="ironpad-toast">
                            <div class=title_class>{t.title}</div>
                            <div class="ironpad-toast-body">{t.body}</div>
                        </div>
                    }
                }
            />
        </div>
    }
}

#[cfg(test)]
mod tests {
    use leptos::reactive::owner::Owner;

    use super::*;

    #[test]
    fn toast_appends_entries_with_distinct_ids() {
        Owner::new().with(|| {
            let toaster = Toaster::new();
            toaster.toast(ToastIntent::Success, "Saved", "All good.", 3);
            toaster.toast(ToastIntent::Error, "Failed", "Not good.", 5);

            toaster.toasts.with_untracked(|list| {
                assert_eq!(list.len(), 2);
                assert_ne!(list[0].id, list[1].id);
                assert_eq!(list[0].title, "Saved");
                assert_eq!(list[0].intent, ToastIntent::Success);
                assert_eq!(list[1].body, "Not good.");
                assert_eq!(list[1].intent, ToastIntent::Error);
            });
        });
    }

    #[test]
    fn toast_ids_stay_unique_after_dismissal() {
        Owner::new().with(|| {
            let toaster = Toaster::new();
            toaster.toast(ToastIntent::Success, "a", "", 3);
            toaster.toast(ToastIntent::Success, "b", "", 3);

            // Dismiss the newest entry, then dispatch again: the new id must
            // not collide with the survivor (ids key the <For> diff).
            toaster
                .toasts
                .update(|list| list.retain(|t| t.title == "a"));
            toaster.toast(ToastIntent::Success, "c", "", 3);

            toaster.toasts.with_untracked(|list| {
                assert_eq!(list.len(), 2);
                assert_ne!(list[0].id, list[1].id);
            });
        });
    }
}
