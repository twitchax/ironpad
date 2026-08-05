//! Session panel UI for starting/stopping agent sessions and viewing tokens.

use leptos::prelude::*;

use crate::session::{ConnectionStatus, SessionState};

// ── Session toolbar button ──────────────────────────────────────────────────

/// Toolbar button that shows session status and toggles the session panel.
#[component]
pub fn SessionButton() -> impl IntoView {
    let session = expect_context::<SessionState>();
    let panel_open = RwSignal::new(false);

    let button_label = Signal::derive(move || {
        if !session.active.get() {
            return "Start Agent Session".to_string();
        }
        let count = session.connected_guests.get().len();
        match session.connection_status.get() {
            ConnectionStatus::Connected => {
                if count > 0 {
                    format!("{count} agent{}", if count == 1 { "" } else { "s" })
                } else {
                    "Session active".to_string()
                }
            }
            ConnectionStatus::Connecting => "Connecting...".to_string(),
            ConnectionStatus::Disconnected => "Disconnected".to_string(),
        }
    });

    let button_class = Signal::derive(move || {
        let mut class = "ironpad-session-button".to_string();
        if session.active.get() {
            class.push_str(" ironpad-session-button--active");
        }
        class
    });

    view! {
        <div class="ironpad-session-wrapper">
            <button
                class=button_class
                on:click=move |_| {
                    if session.active.get() {
                        panel_open.update(|v| *v = !*v);
                    } else {
                        start_session_flow();
                        panel_open.set(true);
                    }
                }
            >
                {button_label}
            </button>

            {move || {
                if panel_open.get() && session.active.get() {
                    view! { <SessionPanel on_close=move || panel_open.set(false) /> }.into_any()
                } else {
                    view! { <div /> }.into_any()
                }
            }}
        </div>
    }
}

// ── Session panel ───────────────────────────────────────────────────────────

/// Expanded panel showing token, connected agents, and controls.
#[component]
fn SessionPanel(on_close: impl Fn() + 'static + Clone) -> impl IntoView {
    let session = expect_context::<SessionState>();
    let token_visible = RwSignal::new(false);

    let token_display = Signal::derive(move || {
        let token = session.token.get().unwrap_or_default();
        if token_visible.get() {
            token
        } else {
            "*".repeat(token.len().min(16))
        }
    });

    let on_close_clone = on_close.clone();

    view! {
        <div class="ironpad-session-panel">
            <div class="ironpad-session-panel-header">
                <span class="ironpad-session-panel-title">"Agent Session"</span>
                <button
                    class="ironpad-session-panel-close"
                    on:click=move |_| on_close_clone()
                >"x"</button>
            </div>

            // ── Token section ────────────────────────────────────────────
            <div class="ironpad-session-token-section">
                <label class="ironpad-session-label">"Token"</label>
                <div class="ironpad-session-token-row">
                    <code class="ironpad-session-token">{token_display}</code>
                    <button
                        class="ironpad-session-token-toggle"
                        on:click=move |_| token_visible.update(|v| *v = !*v)
                    >
                        {move || if token_visible.get() { "Hide" } else { "Show" }}
                    </button>
                    <button
                        class="ironpad-session-token-copy"
                        on:click=move |_| {
                            #[cfg(feature = "hydrate")]
                            if let Some(token) = session.token.get_untracked() {
                                copy_to_clipboard(&token);
                            }
                        }
                    >
                        "Copy"
                    </button>
                </div>
            </div>

            // ── CLI one-liner ────────────────────────────────────────────
            {move || {
                session.token.get().map(|token| {
                    let cmd = cli_command(&ws_origin(), &token);
                    view! {
                        <div class="ironpad-session-cli-section">
                            <label class="ironpad-session-label">"CLI command"</label>
                            <code class="ironpad-session-cli-cmd">{cmd.clone()}</code>
                            <button
                                class="ironpad-session-token-copy"
                                on:click=move |_| {
                                    #[cfg(feature = "hydrate")]
                                    copy_to_clipboard(&cmd);
                                }
                            >
                                "Copy"
                            </button>
                        </div>
                    }
                })
            }}

            // ── Connected agents ─────────────────────────────────────────
            <div class="ironpad-session-agents-section">
                <label class="ironpad-session-label">"Connected agents"</label>
                {move || {
                    let guests = session.connected_guests.get();
                    if guests.is_empty() {
                        view! {
                            <span class="ironpad-session-no-agents">"No agents connected"</span>
                        }.into_any()
                    } else {
                        view! {
                            <div class="ironpad-session-agent-list">
                                {guests.into_iter().map(|id| view! {
                                    <span class="ironpad-tag">{id}</span>
                                }).collect_view()}
                            </div>
                        }.into_any()
                    }
                }}
            </div>

            // ── End session ──────────────────────────────────────────────
            <div class="ironpad-session-actions">
                <button
                    class="ironpad-btn ironpad-btn--primary"
                    on:click=move |_| {
                        end_session_flow();
                    }
                >
                    "End Session"
                </button>
            </div>
        </div>
    }
}

// ── Actions ─────────────────────────────────────────────────────────────────

fn start_session_flow() {
    #[cfg(feature = "hydrate")]
    {
        let session = expect_context::<SessionState>();
        let model = expect_context::<crate::model::NotebookModel>();
        let nb_state = expect_context::<crate::pages::notebook_editor::state::NotebookState>();
        let notebook_id = nb_state.notebook_id.get_untracked();

        if let Err(e) = crate::session::start_session(
            &notebook_id,
            ironpad_common::protocol::Permissions::default(),
            model,
            session,
            nb_state,
        ) {
            web_sys::console::error_1(&format!("Failed to start session: {e}").into());
        }
    }
}

fn end_session_flow() {
    #[cfg(feature = "hydrate")]
    {
        let session = expect_context::<SessionState>();
        crate::session::end_session(&session);
    }
}

// ── CLI command helpers ─────────────────────────────────────────────────────

/// The copy-paste daemon invocation shown in the panel. Must match the real
/// clap surface in `ironpad-cli` (`--host`/`--token` are global flags,
/// `daemon` is the subcommand — there IS no `connect`); this string shipped
/// wrong once and the one command the UI taught failed on paste.
fn cli_command(ws_host: &str, token: &str) -> String {
    format!("ironpad-cli --host {ws_host} --token {token} daemon")
}

/// The current page's origin as a WebSocket URL (`wss://` under TLS).
fn ws_origin() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        let location = web_sys::window().map(|w| w.location());
        let protocol = location
            .as_ref()
            .and_then(|l| l.protocol().ok())
            .unwrap_or_default();
        let host = location
            .as_ref()
            .and_then(|l| l.host().ok())
            .unwrap_or_default();
        ws_origin_from(&protocol, &host)
    }
    #[cfg(not(target_arch = "wasm32"))]
    ws_origin_from("http:", "localhost:3111")
}

/// Map an `http(s):` page protocol + host to the matching WS origin.
fn ws_origin_from(protocol: &str, host: &str) -> String {
    let scheme = if protocol == "https:" { "wss" } else { "ws" };
    format!("{scheme}://{host}")
}

// ── Clipboard helper ────────────────────────────────────────────────────────

#[cfg(feature = "hydrate")]
fn copy_to_clipboard(text: &str) {
    if let Some(window) = web_sys::window() {
        let clipboard = window.navigator().clipboard();
        let _ = clipboard.write_text(text);
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_command_matches_the_real_clap_surface() {
        let cmd = cli_command("wss://ironpad.twitchax.com", "deadbeef");
        assert_eq!(
            cmd,
            "ironpad-cli --host wss://ironpad.twitchax.com --token deadbeef daemon"
        );
        // The subcommand that never existed must never come back.
        assert!(!cmd.contains("connect"));
    }

    #[test]
    fn ws_origin_tracks_the_page_protocol() {
        assert_eq!(
            ws_origin_from("https:", "ironpad.twitchax.com"),
            "wss://ironpad.twitchax.com"
        );
        assert_eq!(
            ws_origin_from("http:", "localhost:3111"),
            "ws://localhost:3111"
        );
    }
}
