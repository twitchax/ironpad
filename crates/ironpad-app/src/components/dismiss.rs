//! Outside-click dismissal for the popovers, dropdowns, and panels that float
//! over the notebook.
//!
//! Each caller names the element that counts as "inside" with a CSS selector
//! and gets one document-level `click` listener, detached on that component's
//! cleanup. The two hard-won details (dispatch-time path capture and listener
//! teardown) live here once rather than at every call site.

/// Invoke `on_outside` when a click lands outside every element matching
/// `selector`.
///
/// Call this from a component's setup. The listener is removed when that
/// component is cleaned up, so remounts never stack stale handlers.
///
/// The selector should name the *wrapper* that contains both the trigger and
/// the floating content, so clicking the trigger reaches its own toggle
/// handler instead of being treated as a dismissal.
#[cfg(feature = "hydrate")]
pub fn dismiss_on_outside_click(selector: &'static str, on_outside: impl Fn() + 'static) {
    use leptos::prelude::*;
    use wasm_bindgen::prelude::*;

    let closure = Closure::<dyn Fn(web_sys::MouseEvent)>::new(move |e: web_sys::MouseEvent| {
        // `composedPath()` rather than `target.closest(...)`: the path is
        // captured at DISPATCH, so it survives the target being re-rendered
        // out of the DOM by an earlier handler on the same click. A menu item
        // that toggles state does exactly that now that items render reactive
        // icon markup (PRD-0062) — the click landed on the icon's span, the
        // toggle replaced it, and `closest()` on the detached node reported
        // "outside", closing the menu out from under its own item.
        let inside = e.composed_path().iter().any(|node| {
            node.dyn_into::<web_sys::Element>()
                .is_ok_and(|el| el.matches(selector).unwrap_or(false))
        });
        if !inside {
            on_outside();
        }
    });

    let Some(document) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    if document
        .add_event_listener_with_callback("click", closure.as_ref().unchecked_ref())
        .is_err()
    {
        return;
    }

    // Hold the closure for as long as the listener refers to it, and detach on
    // unmount so a remount does not stack a second handler on the document.
    let stored = StoredValue::new_local(closure);
    on_cleanup(move || {
        if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
            stored.try_with_value(|closure| {
                let _ = doc
                    .remove_event_listener_with_callback("click", closure.as_ref().unchecked_ref());
            });
        }
    });
}

/// No-op on the server: there is no document to listen on, and callers should
/// not have to gate the call site on the target.
#[cfg(not(feature = "hydrate"))]
pub fn dismiss_on_outside_click(_selector: &'static str, _on_outside: impl Fn() + 'static) {}

/// Clear an open-flag, but only when it is actually set.
///
/// Dismissal callbacks run on *every* click in the document, and an
/// unconditional `set` notifies subscribers whether or not the value moved. A
/// bare `flag.set(false)` therefore re-runs the render closure that flag gates
/// on every click anywhere in the app, forever. `try_get_untracked` keeps the
/// read safe if the owning component has already been disposed.
pub fn clear_if_open(flag: leptos::prelude::RwSignal<bool>) {
    use leptos::prelude::*;

    if flag.try_get_untracked() == Some(true) {
        flag.set(false);
    }
}
