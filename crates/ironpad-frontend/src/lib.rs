//! WASM entry point for the ironpad frontend.
//!
//! Hydrates the server-rendered [`ironpad_app::App`] into an interactive Leptos
//! client. This is the minimal shim compiled to `wasm32` and loaded by the
//! browser; all UI logic lives in `ironpad-app`.

#[wasm_bindgen::prelude::wasm_bindgen]
pub fn hydrate() {
    console_error_panic_hook::set_once();
    // Not `hydrate_body` directly: the shell's scripts are deferred, so mounting
    // has to wait for them. See `ironpad_app::hydrate_body_when_shell_ready`.
    ironpad_app::hydrate_body_when_shell_ready();
}
