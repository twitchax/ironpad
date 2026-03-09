/// Monaco editor Leptos wrapper component.
///
/// Renders a container `<div>`, and on mount (client-side only) creates a
/// Monaco editor instance via the `window.IronpadMonaco` JS bridge.
/// Cleans up on unmount by calling `dispose`.
///
/// Accepts `initial_value`, `language`, and an optional `on_change` callback.
/// Exposes imperative `get_value()` / `set_value()` via [`MonacoEditorHandle`],
/// which can be received through the optional `handle` signal prop.
use leptos::prelude::*;

// ── JS interop (client-side only) ───────────────────────────────────────────

#[cfg(feature = "hydrate")]
mod js {
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen]
    extern "C" {
        /// Create a Monaco editor inside `container`.
        /// Returns a numeric editor ID.
        #[wasm_bindgen(js_namespace = IronpadMonaco)]
        pub fn create(
            container: &web_sys::HtmlElement,
            value: &str,
            language: &str,
            on_change: &js_sys::Function,
        ) -> f64;

        /// Read the current content of the editor identified by `id`.
        #[wasm_bindgen(js_namespace = IronpadMonaco, js_name = "getValue")]
        pub fn get_value(id: f64) -> String;

        /// Overwrite the content of the editor identified by `id`.
        #[wasm_bindgen(js_namespace = IronpadMonaco, js_name = "setValue")]
        pub fn set_value(id: f64, value: &str);

        /// Register a keybinding action on the editor identified by `id`.
        #[wasm_bindgen(js_namespace = IronpadMonaco, js_name = "addAction")]
        pub fn add_action(
            id: f64,
            action_id: &str,
            keybindings: &js_sys::Array,
            callback: &js_sys::Function,
        );

        /// Focus the editor identified by `id`.
        #[wasm_bindgen(js_namespace = IronpadMonaco)]
        pub fn focus(id: f64);

        /// Set model markers (inline error/warning decorations) on the editor.
        /// `markers` is a JS array of marker objects.
        #[wasm_bindgen(js_namespace = IronpadMonaco, js_name = "setMarkers")]
        pub fn set_markers(id: f64, markers: &js_sys::Array);

        /// Clear all ironpad markers from the editor.
        #[wasm_bindgen(js_namespace = IronpadMonaco, js_name = "clearMarkers")]
        pub fn clear_markers(id: f64);

        /// Set per-editor cell context for the completion provider.
        #[wasm_bindgen(js_namespace = IronpadMonaco, js_name = "setCellContext")]
        pub fn set_cell_context(id: f64, context: &wasm_bindgen::JsValue);

        /// Set or unset read-only mode on the editor.
        #[wasm_bindgen(js_namespace = IronpadMonaco, js_name = "setReadOnly")]
        pub fn set_read_only(id: f64, read_only: bool);

        /// Dispose the editor identified by `id`, freeing resources.
        #[wasm_bindgen(js_namespace = IronpadMonaco)]
        pub fn dispose(id: f64);
    }
}

// ── Editor handle ───────────────────────────────────────────────────────────

/// Opaque handle exposing imperative `get_value` / `set_value` methods
/// for a mounted Monaco editor.  Only functional on the client; on SSR
/// all operations are silent no-ops.
#[derive(Clone, Copy)]
pub struct MonacoEditorHandle {
    editor_id: RwSignal<Option<f64>>,
}

impl MonacoEditorHandle {
    /// Read the current editor content.  Returns an empty string when
    /// the editor is not yet mounted or during SSR.
    pub fn get_value(&self) -> String {
        #[cfg(feature = "hydrate")]
        {
            if let Some(id) = self.editor_id.get_untracked() {
                return js::get_value(id);
            }
        }
        String::new()
    }

    /// Overwrite the editor content.  No-op during SSR or before mount.
    pub fn set_value(&self, value: &str) {
        #[cfg(feature = "hydrate")]
        {
            if let Some(id) = self.editor_id.get_untracked() {
                js::set_value(id, value);
            }
        }

        // Suppress unused-variable warning during SSR build.
        #[cfg(not(feature = "hydrate"))]
        let _ = value;
    }

    /// Focus the editor.  No-op during SSR or before mount.
    pub fn focus(&self) {
        #[cfg(feature = "hydrate")]
        {
            if let Some(id) = self.editor_id.get_untracked() {
                js::focus(id);
            }
        }
    }

    /// Register a keybinding action on the editor.
    ///
    /// `keybindings` uses Monaco's numeric keybinding constants
    /// (e.g. `KeyMod.Shift | KeyCode.Enter` = 1027).
    /// Only available in the `hydrate` (client-side) build.
    #[cfg(feature = "hydrate")]
    pub fn add_action(&self, action_id: &str, keybindings: &[i32], callback: &js_sys::Function) {
        if let Some(id) = self.editor_id.get_untracked() {
            let kb_array = js_sys::Array::new();
            for &kb in keybindings {
                kb_array.push(&wasm_bindgen::JsValue::from(kb));
            }
            js::add_action(id, action_id, &kb_array, callback);
        }
    }

    /// Set inline markers (errors/warnings) on the editor model.
    ///
    /// `markers` is a JS array of marker objects with fields:
    /// `startLineNumber`, `startColumn`, `endLineNumber`, `endColumn`,
    /// `message`, `severity` (1=Hint, 2=Info, 4=Warning, 8=Error).
    /// Only available in the `hydrate` (client-side) build.
    #[cfg(feature = "hydrate")]
    pub fn set_markers(&self, markers: &js_sys::Array) {
        if let Some(id) = self.editor_id.get_untracked() {
            js::set_markers(id, markers);
        }
    }

    /// Clear all inline markers from the editor model.
    /// No-op during SSR or before mount.
    pub fn clear_markers(&self) {
        #[cfg(feature = "hydrate")]
        {
            if let Some(id) = self.editor_id.get_untracked() {
                js::clear_markers(id);
            }
        }
    }

    /// Set or unset read-only mode on the editor.
    /// No-op during SSR or before mount.
    pub fn set_read_only(&self, read_only: bool) {
        #[cfg(feature = "hydrate")]
        {
            if let Some(id) = self.editor_id.get_untracked() {
                js::set_read_only(id, read_only);
            }
        }

        // Suppress unused-variable warning during SSR build.
        #[cfg(not(feature = "hydrate"))]
        let _ = read_only;
    }

    /// Set per-editor cell context for the autocomplete provider.
    /// `context` is a JS object with `{ variables: [{name, type, doc}] }`.
    /// No-op during SSR or before mount.
    #[cfg(feature = "hydrate")]
    pub fn set_cell_context(&self, context: &wasm_bindgen::JsValue) {
        if let Some(id) = self.editor_id.get_untracked() {
            js::set_cell_context(id, context);
        }
    }
}

// ── Component ───────────────────────────────────────────────────────────────

/// Leptos component wrapping a Monaco editor instance.
///
/// Monaco is loaded asynchronously via the AMD loader configured in
/// `public/monaco/init.js`; the editor is only created client-side
/// (SSR renders an empty container `<div>`).
#[component]
pub fn MonacoEditor(
    /// Initial text content of the editor.
    #[prop(into)]
    initial_value: String,
    /// Language mode (e.g. `"rust"`, `"toml"`).  Defaults to `"rust"`.
    #[prop(into, default = "rust".into())]
    language: String,
    /// Fires whenever the editor content changes, providing the full new value.
    #[prop(optional)]
    on_change: Option<Callback<String>>,
    /// If provided, the component writes a [`MonacoEditorHandle`] into this
    /// signal once the editor is mounted so the parent can call
    /// `get_value()` / `set_value()` imperatively.
    #[prop(optional)]
    handle: Option<RwSignal<Option<MonacoEditorHandle>>>,
    /// If true, the editor is set to read-only mode after creation.
    #[prop(optional)]
    read_only: bool,
) -> impl IntoView {
    let container_ref = NodeRef::new();
    let editor_id: RwSignal<Option<f64>> = RwSignal::new(None);

    // Initialise the Monaco editor on the client only.  `Effect::new`
    // does not execute during SSR, so this is inherently SSR-safe.

    #[cfg(feature = "hydrate")]
    {
        use std::ops::Deref;
        use wasm_bindgen::prelude::*;

        let iv = initial_value.clone();
        let lang = language.clone();

        Effect::new(move || {
            // Wait for the container DOM node to appear.
            let container: Option<web_sys::HtmlDivElement> = container_ref.get_untracked();
            let Some(container) = container else {
                return;
            };

            // Guard against double-initialisation if the effect re-runs.
            if editor_id.get_untracked().is_some() {
                return;
            }

            // Build the on-change JS callback.
            let on_change_fn = match on_change {
                Some(cb) => {
                    let closure = Closure::<dyn Fn(String)>::new(move |val: String| {
                        cb.run(val);
                    });
                    let f: js_sys::Function =
                        closure.as_ref().unchecked_ref::<js_sys::Function>().clone();
                    // Prevent the closure from being dropped while the editor lives.
                    // It will be freed when the editor is disposed (page navigation).
                    closure.forget();
                    f
                }
                None => js_sys::Function::new_no_args(""),
            };

            // Create the editor via the JS bridge.
            let el: &web_sys::HtmlElement = container.deref();
            let id = js::create(el, &iv, &lang, &on_change_fn);
            editor_id.set(Some(id));

            // Apply read-only mode if requested.
            if read_only {
                js::set_read_only(id, true);
            }

            // Publish the handle so the parent can call get_value/set_value.
            if let Some(h) = handle {
                h.set(Some(MonacoEditorHandle { editor_id }));
            }
        });

        on_cleanup(move || {
            if let Some(id) = editor_id.get_untracked() {
                js::dispose(id);
            }
        });
    }

    // Suppress unused warnings during SSR.
    #[cfg(not(feature = "hydrate"))]
    {
        let _ = (
            &initial_value,
            &language,
            &on_change,
            &handle,
            &container_ref,
            &editor_id,
            &read_only,
        );
    }

    view! {
        <div class="ironpad-monaco-container" node_ref=container_ref></div>
    }
}
