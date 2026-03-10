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

    let on_click = move |_| {
        let _ = &copied;
        let _ = &text;

        #[cfg(feature = "hydrate")]
        {
            use wasm_bindgen::prelude::*;

            let window = web_sys::window().unwrap();
            let clipboard = window.navigator().clipboard();
            let _ = clipboard.write_text(&text.get_value());

            copied.set(true);

            let reset = Closure::<dyn Fn()>::new(move || {
                copied.set(false);
            });
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(
                reset.as_ref().unchecked_ref(),
                1_500,
            );
            reset.forget();
        }
    };

    view! {
        <button
            class="ironpad-copy-btn"
            on:click=on_click
            title="Copy to clipboard"
        >
            <span class="ironpad-copy-btn__icon">"📋"</span>
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
