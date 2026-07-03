use leptos::prelude::*;

/// A small copy-to-clipboard button with "Copied!" feedback.
///
/// Positioned absolutely — the parent element must have `position: relative`.
#[component]
pub fn CopyButton(
    /// The text to copy to the clipboard on click.
    #[prop(into)]
    text: String,
) -> impl IntoView {
    let copied = RwSignal::new(false);
    let text = StoredValue::new(text);
    // Pending "reset to un-copied" timeout id — tracked so we can clear it on
    // unmount and on a re-click, instead of leaking closures / stacking timers.
    let timeout_id = RwSignal::new(Option::<i32>::None);
    #[cfg(feature = "hydrate")]
    let reset_holder =
        StoredValue::new_local(Option::<wasm_bindgen::closure::Closure<dyn Fn()>>::None);

    let on_click = move |_| {
        let _ = &copied;
        let _ = &text;

        #[cfg(feature = "hydrate")]
        {
            use wasm_bindgen::prelude::*;

            let window = web_sys::window().unwrap();
            let clipboard = window.navigator().clipboard();
            let _ = clipboard.write_text(&text.get_value());

            // Cancel a still-pending reset so a rapid re-click doesn't hide
            // "Copied!" early and timers don't stack.
            if let Some(id) = timeout_id.get_untracked() {
                window.clear_timeout_with_handle(id);
            }

            copied.set(true);

            let reset = Closure::<dyn Fn()>::new(move || {
                copied.set(false);
            });
            timeout_id.set(
                window
                    .set_timeout_with_callback_and_timeout_and_arguments_0(
                        reset.as_ref().unchecked_ref(),
                        1_500,
                    )
                    .ok(),
            );
            // Hold the closure in the arena (dropped on unmount / on the next
            // click) rather than leaking it with forget().
            reset_holder.set_value(Some(reset));
        }
    };

    #[cfg(feature = "hydrate")]
    on_cleanup(move || {
        if let (Some(id), Some(win)) = (timeout_id.get_untracked(), web_sys::window()) {
            win.clear_timeout_with_handle(id);
        }
    });

    view! {
        <button
            class="ironpad-copy-btn"
            on:click=on_click
            title="Copy to clipboard"
        >
            <span class="ironpad-copy-btn__icon">"⧉"</span>
            <span class=move || {
                if copied.get() {
                    "ironpad-copy-btn__feedback ironpad-copy-btn__feedback--visible"
                } else {
                    "ironpad-copy-btn__feedback"
                }
            }>"Copied!"</span>
        </button>
    }
}
