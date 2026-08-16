//! Linux cells: a cell that is a real Linux process (PRD-0066).
//!
//! A `CellType::Linux` cell is a whole Rust program — the author writes
//! `fn main()` — compiled to `wasm32-browserpod-linux-musl` and run under
//! `BrowserPod`'s in-browser kernel, which services 431 syscalls against the
//! dozen host functions our own executor provides. It gets a filesystem,
//! subprocesses, sockets and real threads, and it shares all of them with
//! every other Linux cell in the notebook, because there is ONE POD PER
//! NOTEBOOK and that shared filesystem is the piping model. No `cellN`
//! bindings, no type tags, no `last`.
//!
//! Three things make this cell unlike every other one:
//!
//! **It never autoruns.** Every boot spends a metered token on the instance
//! owner's allowance, so a boot must trace back to a click. That is not
//! enforced here by a check; it is true by construction, because
//! `IronpadCell::is_runnable` is false for a Linux cell and the run-all queue,
//! the public-notebook autorun and the reactive queue are all built from it.
//! This component never reads `run_all_queue` at all.
//!
//! **It streams.** Every other panel is a finished value rendered once, and
//! `PanelMode::Snapshot` assumes that finality. A Linux cell's output arrives
//! as bytes over time from a pty, so it gets its own panel: a terminal that
//! appends. It also never captures `saved_output` — a stream has no last
//! frame worth persisting, and a reader gets a Run affordance instead.
//!
//! **It carries attribution.** `BrowserPod`'s free tier requires a visible link
//! and logo, so every Linux cell frame header renders one, and nothing else in
//! ironpad does.
//!
//! The pod itself lives in `public/browserpod-runtime.js`, which is loaded ON
//! DEMAND from [`ensure_runtime`] at the moment someone clicks Run. A notebook
//! with no Linux cell must never touch their CDN, and whether a page has a
//! Linux cell is a property of its content rather than its route — so this
//! cannot be a `script-loader.js` route entry and is not one.

use leptos::prelude::*;

use ironpad_common::IronpadCell;

use crate::components::icon::{Chevron, Icon, IconLabel};
use crate::components::icons;
use crate::components::monaco_editor::MonacoEditor;

// ── Pod runtime bindings (client-side only) ─────────────────────────────────

/// The on-demand runtime script, loaded by [`ensure_runtime`].
#[cfg(feature = "hydrate")]
const RUNTIME_SCRIPT: &str = "/browserpod-runtime.js";

#[cfg(feature = "hydrate")]
mod js {
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen]
    extern "C" {
        /// Whether the runtime already holds this instance's API key.
        #[wasm_bindgen(js_namespace = IronpadPod, js_name = "hasKey")]
        pub fn has_key() -> bool;

        /// Hand the runtime the key fetched from the server.
        #[wasm_bindgen(js_namespace = IronpadPod)]
        pub fn configure(api_key: &str);

        /// A Linux cell mounted. Boots nothing; only refcounts the pod.
        #[wasm_bindgen(js_namespace = IronpadPod)]
        pub fn retain(notebook_id: &str, cell_id: &str);

        /// A Linux cell unmounted. The pod goes when the last one does.
        #[wasm_bindgen(js_namespace = IronpadPod)]
        pub fn release(cell_id: &str);

        /// Drop the pod. The only lever there is over a running program,
        /// since `run()` hands back a bare PID with no `kill`.
        #[wasm_bindgen(js_namespace = IronpadPod)]
        pub fn teardown();

        /// Boot if needed, write the binary, run it, stream its output.
        /// Resolves with `{status, exitCode, inferred}`.
        #[wasm_bindgen(js_namespace = IronpadPod, catch)]
        pub fn run(opts: &js_sys::Object) -> Result<js_sys::Promise, JsValue>;
    }
}

/// Whether `browserpod-runtime.js` has executed on this page.
///
/// wasm-bindgen resolves a `js_namespace` at CALL time, so every binding in
/// [`js`] is a bare `IronpadPod.foo()` reference that throws until the script
/// has run. Mount-time and cleanup-time calls therefore have to ask first;
/// the run path instead awaits [`ensure_runtime`].
#[cfg(feature = "hydrate")]
fn runtime_loaded() -> bool {
    web_sys::window().is_some_and(|w| {
        js_sys::Reflect::get(&w, &wasm_bindgen::JsValue::from_str("IronpadPod"))
            .is_ok_and(|v| !v.is_undefined() && !v.is_null())
    })
}

/// Append the runtime's `<script>` and memoise its load promise on `window`.
///
/// Resolves on `load` and rejects on `error`; whether the runtime is actually
/// there afterwards is [`ensure_runtime`]'s question, not this one's.
#[cfg(feature = "hydrate")]
fn append_runtime_script(
    window: &web_sys::Window,
    slot: &wasm_bindgen::JsValue,
) -> Result<js_sys::Promise, String> {
    use wasm_bindgen::{closure::Closure, JsValue};

    let document = window.document().ok_or("no document")?;
    let src = crate::versioned(RUNTIME_SCRIPT);
    let promise = js_sys::Promise::new(&mut |resolve, reject| {
        let Ok(element) = document.create_element("script") else {
            let _ = reject.call0(&JsValue::NULL);
            return;
        };
        let _ = element.set_attribute("src", &src);
        // `onload`/`onerror` through Reflect rather than the typed setters:
        // `HtmlScriptElement` is not among this crate's web-sys features, and
        // adding it for two property writes is not worth the compile time.
        let on_load = Closure::once_into_js(move || {
            let _ = resolve.call0(&JsValue::NULL);
        });
        let on_error = Closure::once_into_js(move || {
            let _ = reject.call0(&JsValue::NULL);
        });
        let _ = js_sys::Reflect::set(&element, &JsValue::from_str("onload"), &on_load);
        let _ = js_sys::Reflect::set(&element, &JsValue::from_str("onerror"), &on_error);
        if let Some(head) = document.head() {
            let _ = head.append_child(&element);
        }
    });
    let _ = js_sys::Reflect::set(window, slot, &promise);
    Ok(promise)
}

/// Load `browserpod-runtime.js`, once per page.
///
/// Memoised through a promise parked on `window`, rather than in Rust, because
/// two cells can click Run in the same tick and a second script tag would run
/// the runtime's IIFE again. (The IIFE guards re-entry as well — this is the
/// belt to that suspenders, and it also saves the duplicate fetch.)
///
/// This is a same-origin script. It is the runtime's own dynamic `import()`
/// that reaches rt.browserpod.io, and that only happens on a boot.
///
/// Success means [`runtime_loaded`], never merely that a `load` event fired.
/// A script tag fires `load` for any 200 — an SPA HTML fallback served mid
/// deploy, a body that threw on its first line — and the very next thing the
/// caller does is a non-`catch` binding on `IronpadPod`, which throws out of
/// the wasm frame and leaves the run's future to never complete.
///
/// The memo is CLEARED on failure, which is the other half: parked on `window`
/// it outlives everything, so one transient fetch error otherwise answered
/// every Linux cell on the page, for the life of the page, without ever
/// retrying.
#[cfg(feature = "hydrate")]
async fn ensure_runtime() -> Result<(), String> {
    use wasm_bindgen::{JsCast, JsValue};

    const FAILED: &str = "Could not load the Linux cell runtime from this ironpad instance.";

    // The global is the real precondition; the script tag is only how it gets
    // there.
    if runtime_loaded() {
        return Ok(());
    }

    let window = web_sys::window().ok_or("no window")?;
    let slot = JsValue::from_str("__ironpadPodScript");

    let memoised = js_sys::Reflect::get(&window, &slot)
        .ok()
        .and_then(|existing| existing.dyn_into::<js_sys::Promise>().ok());

    let promise = if let Some(promise) = memoised {
        promise
    } else {
        append_runtime_script(&window, &slot)?
    };

    let fetched = wasm_bindgen_futures::JsFuture::from(promise).await.is_ok();
    if fetched && runtime_loaded() {
        return Ok(());
    }
    // Undefined rather than deleted: the read above already treats anything
    // that is not a promise as "no attempt yet", so the next click appends a
    // fresh tag and tries again.
    let _ = js_sys::Reflect::set(&window, &slot, &JsValue::UNDEFINED);
    Err(FAILED.to_string())
}

// ── Run state ───────────────────────────────────────────────────────────────

/// Where a Linux cell is in its one-click lifecycle.
///
/// `Booting` is its own stage because it is the expensive, metered one (a cold
/// pod is 1.5-2s) and because it is where a CDN failure surfaces; a reader
/// staring at "compiling" while the real wait is a machine boot would be told
/// the wrong thing about why.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Stage {
    #[default]
    Idle,
    Compiling,
    Booting,
    Running,
    /// Past the runtime's reporting horizon: still streaming, with nothing
    /// left that could say when it finishes. A separate stage rather than an
    /// outcome because the run is not over — the panel says so, and the only
    /// control that can stop it has to still be there when it does.
    Unknown,
    Done,
}

impl Stage {
    /// Whether a run is in flight, which is what puts Terminate in the header
    /// in place of Run.
    ///
    /// `Unknown` belongs here for a reason worth stating: the panel tells the
    /// reader that output keeps arriving and that stopping it means shutting
    /// the machine down, and that sentence is only true while the control it
    /// describes exists.
    fn busy(self) -> bool {
        matches!(
            self,
            Self::Compiling | Self::Booting | Self::Running | Self::Unknown
        )
    }

    /// The header pill. Empty where there is nothing in flight to describe.
    fn label(self) -> &'static str {
        match self {
            Self::Compiling => "compiling",
            // Named for what is actually happening and what it costs: this is
            // the metered step, and it is where an unreachable CDN shows up.
            Self::Booting => "starting machine",
            Self::Running => "running",
            Self::Unknown => "still running",
            Self::Idle | Self::Done => "",
        }
    }
}

/// How a run ended, as far as anything can tell from outside the pod.
#[derive(Clone, Debug)]
struct Outcome {
    /// `exited` (the shell sentinel reported a real status), `finished`
    /// (inferred from a quiet terminal), `terminated` (the pod went) or
    /// `cancelled` (stopped during the compile, before any machine existed —
    /// the one status minted here rather than by the runtime).
    status: String,
    exit_code: Option<i32>,
    /// Whether `status` was inferred rather than reported. Surfaced to the
    /// reader, because "we think it finished" and "it exited 0" are different
    /// claims and only one of them is evidence.
    inferred: bool,
}

impl Outcome {
    /// One short phrase for the caption row.
    fn note(&self) -> String {
        match (self.status.as_str(), self.exit_code) {
            ("exited", Some(0)) => "exit 0".to_string(),
            ("exited", Some(code)) => format!("exit {code}"),
            ("exited", None) => "exited".to_string(),
            ("finished", _) if self.inferred => "finished (inferred)".to_string(),
            ("finished", _) => "finished".to_string(),
            ("unknown", _) => "still running".to_string(),
            ("terminated", _) => "machine stopped".to_string(),
            ("cancelled", _) => "stopped".to_string(),
            (other, _) => other.to_string(),
        }
    }

    /// Whether this reads as a failure. An unknown or terminated run is not a
    /// failure: nobody has established that it failed.
    fn failed(&self) -> bool {
        self.status == "exited" && self.exit_code.is_some_and(|c| c != 0)
    }

    /// Stopped before any machine existed: Terminate clicked during the
    /// compile, or a newer run superseding this one.
    fn cancelled() -> Self {
        Self {
            status: "cancelled".to_string(),
            exit_code: None,
            inferred: false,
        }
    }

    /// Whether a program ever reached a machine.
    ///
    /// Stopping during the compile ends the run without one, so the cell must
    /// not go on to read as having been run, and its empty terminal must not
    /// claim the program printed nothing.
    fn started(&self) -> bool {
        self.status != "cancelled"
    }
}

/// A run's claim on the cell, as an epoch that Terminate and the next Run both
/// invalidate.
///
/// Terminate has no other lever during `Compiling`: the pod runtime is not
/// even loaded yet, so there is nothing to tear down and the click can only be
/// honoured by the spawned task abandoning itself. What makes this an epoch
/// rather than a flag is the race a flag loses — Stop, then Run again while
/// the first compile is still in flight, would clear the flag and let the
/// abandoned task boot after all.
#[derive(Clone, Copy)]
struct RunGuard {
    epoch: u64,
    current: RwSignal<u64>,
}

impl RunGuard {
    /// Whether this run may still spend a metered pod boot.
    fn may_boot(self) -> bool {
        Self::may_boot_at(self.current.try_get_untracked(), self.epoch)
    }

    /// The decision itself, separated from the signal so the invariant is
    /// testable: a bumped epoch means Terminate or a newer run, and `None`
    /// means the component was disposed mid-compile (a reader who navigated
    /// away is the ordinary way to get there). Both must refuse.
    fn may_boot_at(current: Option<u64>, epoch: u64) -> bool {
        current == Some(epoch)
    }
}

// ── ViewOnlyLinuxCell ───────────────────────────────────────────────────────

/// A Linux cell on a view-only page: read-only source, a Run affordance that
/// boots the notebook's pod on first use, and a streaming terminal panel.
#[allow(unused_variables)] // Several parameters are used only under `hydrate`.
#[component]
pub(crate) fn ViewOnlyLinuxCell(
    cell: IronpadCell,
    index: Option<usize>,
    anchor_id: String,
    shared_cargo_toml: Option<String>,
    shared_source: Option<String>,
    notebook_id: String,
    force_recompile: RwSignal<bool>,
    share_blob: Option<ironpad_common::ShareBlobEntry>,
    /// Rendered inside an `/embed/*` iframe. Static and known at SSR, which is
    /// why the refusal keys on it (see below).
    #[prop(optional)]
    embed: bool,
) -> impl IntoView {
    let cell = StoredValue::new(cell);
    let stored_cargo_toml = StoredValue::new(shared_cargo_toml);
    let stored_source = StoredValue::new(shared_source);
    let stored_notebook_id = StoredValue::new(notebook_id);
    let stored_share_blob = StoredValue::new(share_blob);

    // A pod needs `SharedArrayBuffer` — their kernel runs a Worker per thread
    // over one shared memory — and that needs a cross-origin-isolated page.
    // The app itself always is, because the server sets COOP/COEP globally
    // (it is why rayon cells work), but a CROSS-ORIGIN IFRAME never can be.
    // So an embed refuses Linux cells up front instead of letting the click
    // fail somewhere inside a vendor SDK, which is the same call PRD-0039
    // made for threaded cells and the same reason.
    //
    // Keyed on the `embed` PROP, not on asking the page, because the markup
    // this decides is emitted at SSR and hydration cannot change it. tachys
    // skips attribute writes when hydrating server HTML
    // (`html/attribute/value.rs`: `if !FROM_SERVER { set_attribute(...) }`),
    // so a runtime `crossOriginIsolated` check produced a cross-origin embed
    // serving a Run button WITHOUT `disabled`, labelled as runnable, that did
    // nothing when clicked. It passed every test we had because a same-origin
    // test page IS isolated, so SSR and hydrate agreed and the bug could not
    // fire where we looked.
    //
    // `embed` is known on the server, so both renders agree by construction.
    let runnable_here = !embed;

    // Belt and braces for a self-hosted instance serving `/embed/*` without
    // COOP/COEP: the click path still refuses if the page turns out not to be
    // isolated. That check cannot drive markup, only behaviour.
    #[cfg(feature = "hydrate")]
    let isolated = leptos::web_sys::window()
        .and_then(|w| {
            js_sys::Reflect::get(&w, &"crossOriginIsolated".into())
                .ok()
                .and_then(|v| v.as_bool())
        })
        .unwrap_or(false);
    #[cfg(not(feature = "hydrate"))]
    let isolated = true;

    let stage = RwSignal::new(Stage::Idle);
    let output = RwSignal::new(String::new());
    let error_message: RwSignal<Option<String>> = RwSignal::new(None);
    let outcome: RwSignal<Option<Outcome>> = RwSignal::new(None);
    let compile_time_ms: RwSignal<Option<f64>> = RwSignal::new(None);
    // Bumped by every Run and every Terminate; see [`RunGuard`].
    let run_epoch = RwSignal::new(0u64);

    // The pod is refcounted by mounted Linux cells: the last one to unmount
    // takes the machine with it, after the runtime's grace window. Both calls
    // must be guarded on the runtime being loaded at all, since a page whose
    // Linux cells were never run has no `IronpadPod` global to call into.
    //
    // The retain is what makes a REMOUNT free: the editor's Preview↔Edit
    // toggle disposes this whole subtree and builds it again, and without a
    // claim on the way back in, the release's teardown timer would still be
    // armed and would take the machine — and its filesystem — out from under
    // a cell that is sitting right there.
    #[cfg(feature = "hydrate")]
    {
        let cell_id = cell.with_value(|c| c.id.clone());
        if runtime_loaded() {
            js::retain(&stored_notebook_id.get_value(), &cell_id);
        }
        on_cleanup(move || {
            if runtime_loaded() {
                js::release(&cell_id);
            }
        });
    }

    // ── Run ─────────────────────────────────────────────────────────────
    //
    // Only ever reached from the button below. Nothing watches a queue.

    let run_cell = move |_| {
        #[cfg(feature = "hydrate")]
        {
            if !isolated {
                return; // the notice below says why; never boot from here
            }
            if stage.get_untracked().busy() {
                return;
            }
            stage.set(Stage::Compiling);
            error_message.set(None);
            outcome.set(None);
            output.set(String::new());
            let guard = RunGuard {
                epoch: run_epoch.get_untracked().wrapping_add(1),
                current: run_epoch,
            };
            run_epoch.set(guard.epoch);

            leptos::task::spawn_local(async move {
                // Every `StoredValue` this task needs is read HERE, before the
                // first await. Reading one after the component is disposed
                // panics (writes no-op, reads do not), and a reader navigating
                // away mid-compile is the ordinary way to get there — the same
                // shape as the bug that once aborted the whole wasm app from a
                // viewer's blame lookup.
                let cell_data = cell.get_value();
                let notebook_id = stored_notebook_id.get_value();
                let request = ironpad_common::CompileRequest {
                    notebook_id: notebook_id.clone(),
                    cell_id: cell_data.id.clone(),
                    source: cell_data.source.clone(),
                    cargo_toml: cell_data.cargo_toml.clone().unwrap_or_default(),
                    // A Linux cell consumes no upstream slot: there are no
                    // `cellN` bindings to bind, because the pod's filesystem
                    // is how one program hands another its output.
                    previous_cell_types: Vec::new(),
                    shared_cargo_toml: stored_cargo_toml.get_value(),
                    shared_source: stored_source.get_value(),
                    force: force_recompile.get_untracked(),
                    shared_check: None,
                    // Derived from the cell, never asserted about it. This
                    // component is only ever reached from the `CellType::Linux`
                    // arm of the viewer's dispatch, so a constant would be
                    // correct today — and would be a constant on a compile
                    // request, which is the shape that compiles a program for
                    // the wrong world the moment a route changes underneath it.
                    cell_type: cell_data.cell_type,
                };

                let compile_start = js_sys::Date::now();
                let acquisition = crate::components::run_flow::acquire_blob(
                    &request,
                    stored_share_blob.get_value().as_ref(),
                )
                .await;

                // Cached first, and unconditionally: the compile is paid for
                // whether or not anyone still wants its result, the blob is
                // content-addressed, and an empty one is dropped inside.
                if let Ok(response) = acquisition.result.as_ref() {
                    crate::blob_cache::store_unless_served(
                        acquisition.served_without_server,
                        acquisition.request_hash.as_deref(),
                        response,
                    )
                    .await;
                }

                // Terminated, superseded, or disposed while compiling: stop
                // here, before any signal is touched and long before a boot.
                // A boot spends a metered token off an allowance of roughly a
                // thousand a month, and this is the only place a Stop clicked
                // during the compile can possibly be honoured — the pod
                // runtime is not even loaded yet, so there is nothing for
                // Terminate itself to tear down.
                if !guard.may_boot() {
                    return;
                }

                let response = match acquisition.result {
                    Ok(response) => {
                        compile_time_ms.set(Some(js_sys::Date::now() - compile_start));
                        if response.wasm_blob.is_empty() {
                            let errors: Vec<String> = response
                                .diagnostics
                                .iter()
                                .map(|d| d.message.clone())
                                .collect();
                            error_message.set(Some(errors.join("\n")));
                            stage.set(Stage::Done);
                            return;
                        }
                        response
                    }
                    Err(e) => {
                        error_message.set(Some(format!("Compile error: {e}")));
                        stage.set(Stage::Done);
                        return;
                    }
                };

                stage.set(Stage::Booting);
                match execute_in_pod(
                    &notebook_id,
                    &cell_data.id,
                    &response.wasm_blob,
                    output,
                    stage,
                    guard,
                )
                .await
                {
                    Ok(result) => outcome.set(Some(result)),
                    Err(e) => error_message.set(Some(e)),
                }
                stage.set(Stage::Done);
            });
        }
    };

    // Terminate. There is no `kill` anywhere in the SDK — `run()` resolves to
    // a bare PID — so stopping a program means dropping the machine it runs
    // on, which is why the control says so rather than saying "stop".
    //
    // The machine is only half of it. This button replaces Run the moment the
    // compile starts, and for those 300 seconds there is no machine yet: the
    // epoch bump is what a Stop clicked in that window actually does, and
    // without it the click did nothing at all, silently, and then the compile
    // landed and spent a token booting the pod the reader had just refused.
    let terminate = move |_| {
        #[cfg(feature = "hydrate")]
        {
            run_epoch.update(|e| *e = e.wrapping_add(1));
            if runtime_loaded() {
                js::teardown();
            }
            // A run that has reached the pod reports its own end (the runtime
            // settles it as `terminated`), so only the pre-boot window needs
            // to say what happened here — and it must say something, or the
            // click reads as a dead button.
            if stage.get_untracked() == Stage::Compiling {
                stage.set(Stage::Done);
                outcome.set(Some(Outcome::cancelled()));
            }
        }
    };

    let collapsed = RwSignal::new(cell.with_value(|c| c.collapsed));
    let output_collapsed = RwSignal::new(cell.with_value(|c| c.output_collapsed));
    let body_class = Signal::derive(move || {
        if collapsed.get() {
            "ironpad-cell-body ironpad-cell-body--collapsed"
        } else {
            "ironpad-cell-body"
        }
    });

    let busy = Signal::derive(move || stage.get().busy());
    let stage_label = Signal::derive(move || stage.get().label());
    // The terminal earns its place only when there is a transcript or a run in
    // flight. Keyed off Stage alone it also appeared under an error like "no
    // key configured", pairing a real diagnosis with an empty box claiming the
    // program produced no output.
    let show_terminal = Signal::derive(move || {
        !output.get().is_empty()
            || outcome.get().is_some()
            || matches!(
                stage.get(),
                Stage::Booting | Stage::Running | Stage::Unknown
            )
    });
    let run_button_class = Signal::derive(move || {
        if error_message.get().is_some() || outcome.get().is_some_and(|o| o.failed()) {
            "view-only-run-button view-only-run-button--error"
        } else if outcome.get().is_some_and(|o| o.started()) {
            "view-only-run-button view-only-run-button--ran"
        } else {
            "view-only-run-button"
        }
    });

    view! {
        <div class="view-only-cell view-only-cell--frame view-only-cell--linux" id=anchor_id>
            <div class="view-only-cell-header">
                <button
                    class="ironpad-cell-collapse-btn"
                    on:click=move |_| collapsed.update(|c| *c = !*c)
                >
                    <Chevron expanded=Signal::derive(move || !collapsed.get())/>
                </button>
                {index.map(|n| view! {
                    <span class="view-only-cell-index">{format!("[{n}]")}</span>
                })}
                <span class="view-only-cell-label">{cell.with_value(|c| c.label.clone())}</span>
                <span class="ironpad-cell-type-badge ironpad-cell-type-badge--linux">
                    <IconLabel icon=icons::LINUX label="linux"/>
                </span>
                <LinuxAttribution/>
                {move || (!stage_label.get().is_empty()).then(|| view! {
                    <span class="view-only-pill view-only-pill--busy">{stage_label.get()}</span>
                })}
                <span class="view-only-cell-meta">
                    {move || {
                        // A failed compile still records a duration; the error
                        // panel is the thing to read in that case.
                        if error_message.get().is_some() {
                            return None;
                        }
                        compile_time_ms.get().map(|c| view! {
                            <span class="view-only-timing-badge">{format!("{c:.0}ms compile")}</span>
                        })
                    }}
                </span>
                {move || if busy.get() {
                    view! {
                        <button
                            class="view-only-run-button ironpad-linux-terminate"
                            on:click=terminate
                            title="Stop by shutting down the Linux machine"
                            aria-label="Stop by shutting down the Linux machine"
                        >
                            <Icon icon=icons::STOP/>
                        </button>
                    }.into_any()
                } else {
                    view! {
                        <button
                            class=run_button_class
                            on:click=run_cell
                            disabled=!runnable_here
                            title=if runnable_here {
                                "Run this program on a Linux machine in your browser"
                            } else {
                                "Linux cells cannot run in an embed"
                            }
                            aria-label=if runnable_here {
                                "Run this program on a Linux machine in your browser"
                            } else {
                                "Linux cells cannot run in an embed"
                            }
                        >
                            <Icon icon=icons::RUN/>
                        </button>
                    }.into_any()
                }}
            </div>
            <div class=body_class>
                <MonacoEditor
                    initial_value=cell.with_value(|c| c.source.clone())
                    language="rust"
                    read_only=true
                />
            </div>
            // Sibling of the source pane rather than inside it: a collapsed
            // cell must still say why its Run button is dead.
            {(!runnable_here).then(|| view! {
                <div class="view-only-inert-notice">
                    "Linux cells need a cross-origin-isolated page, which an embed is not. \
                     Open the notebook in ironpad to run this one."
                </div>
            })}
            {move || error_message.get().map(|err| view! {
                <div class="view-only-error">
                    <pre>{err}</pre>
                </div>
            })}
            {move || {
                // Once shown it stays: a finished stream is still the record
                // of what happened.
                if !show_terminal.get() {
                    return None;
                }
                if output_collapsed.get() {
                    return Some(view! {
                        <button
                            class="view-only-output-reveal"
                            on:click=move |_| output_collapsed.set(false)
                        >
                            <span class="ironpad-output-toggle"><Icon icon=icons::CHEVRON_RIGHT/></span>
                            <span>"Terminal"</span>
                        </button>
                    }.into_any());
                }
                Some(view! {
                    <LinuxTerminal output=output stage=stage outcome=outcome/>
                }.into_any())
            }}
        </div>
    }
}

// ── LinuxTerminal ───────────────────────────────────────────────────────────

/// The streaming output panel.
///
/// A new panel kind rather than a `DisplayPanel`: every existing panel is a
/// finished value handed over once, and `PanelMode::Snapshot` is built on that
/// finality. This one is a pty transcript that grows, so it renders raw text
/// in a fixed-height scroller that follows the tail.
///
/// The text arrives from the runtime as a COMPLETE snapshot each time rather
/// than as deltas, which is what keeps carriage-return line rewriting and the
/// buffer cap in one place (the runtime) instead of two.
#[component]
fn LinuxTerminal(
    output: RwSignal<String>,
    stage: RwSignal<Stage>,
    outcome: RwSignal<Option<Outcome>>,
) -> impl IntoView {
    let scroller: NodeRef<leptos::html::Div> = NodeRef::new();

    // Follow the tail, but only while the reader is already at it. Yanking
    // the viewport back while someone is reading earlier output is the one
    // thing a log view must not do.
    #[cfg(feature = "hydrate")]
    {
        Effect::new(move |_| {
            // A run outlives its panel (navigate away mid-stream) and this
            // effect can be queued past disposal, where a plain read panics.
            // `try_get` subscribes exactly as `get` does — the guard below it
            // was written for this and sat one line too low to do it.
            if output.try_get().is_none() {
                return;
            }
            let Some(el) = scroller.try_get_untracked().flatten() else {
                return;
            };
            let el: &web_sys::HtmlElement = &el;
            let bottom = el.scroll_height() - el.scroll_top() - el.client_height();
            if bottom < 48 {
                el.set_scroll_top(el.scroll_height());
            }
        });
    }

    let note = Signal::derive(move || {
        outcome.get().map_or_else(
            || match stage.get() {
                Stage::Booting => "starting the machine".to_string(),
                Stage::Running => "streaming".to_string(),
                Stage::Unknown => "still running".to_string(),
                _ => String::new(),
            },
            |o| o.note(),
        )
    });

    view! {
        <div class="view-only-output ironpad-linux-output">
            <div class="view-only-output-caption">
                <div class="view-only-output-caption-lead">
                    <span class="view-only-output-title">"Terminal"</span>
                </div>
                <div class="view-only-output-note">
                    <span class=move || if outcome.get().is_some_and(|o| o.failed()) {
                        "ironpad-linux-status ironpad-linux-status--error"
                    } else {
                        "ironpad-linux-status"
                    }>{move || note.get()}</span>
                </div>
            </div>
            <div
                class="view-only-output-body ironpad-linux-terminal"
                node_ref=scroller
                tabindex="0"
                role="region"
                aria-label="Terminal output"
            >
                <pre class="ironpad-linux-terminal-text">{move || output.get()}</pre>
                {move || {
                    // An empty transcript is ambiguous — nothing printed yet,
                    // or nothing to print — so it says which.
                    (output.get().is_empty()).then(|| view! {
                        <div class="ironpad-linux-terminal-empty">
                            {move || if stage.get() != Stage::Done {
                                "Waiting for output…"
                            } else if outcome.get().is_some_and(|o| !o.started()) {
                                // Stopped during the compile: no machine ever
                                // existed, so "produced no output" would be
                                // reporting on a program that never ran.
                                "Stopped before the program ran."
                            } else {
                                "The program produced no output."
                            }}
                        </div>
                    })
                }}
            </div>
            // Keyed on the STAGE, not on an outcome: passing the reporting
            // horizon does not end the run. Output keeps arriving and the
            // Terminate control is still there, which is what makes both
            // halves of this sentence true.
            {move || (stage.get() == Stage::Unknown).then(|| view! {
                <div class="view-only-blocked">
                    <Icon icon=icons::WARNING/>
                    " This program has not signalled that it finished. Output keeps arriving; \
                     stopping it means shutting the Linux machine down."
                </div>
            })}
            {move || outcome.get().filter(|o| o.status == "terminated").map(|_| view! {
                <div class="view-only-blocked">
                    <Icon icon=icons::WARNING/>
                    " The Linux machine was shut down. Running any Linux cell starts a fresh \
                     one, and its filesystem starts empty."
                </div>
            })}
        </div>
    }
}

// ── LinuxAttribution ────────────────────────────────────────────────────────

/// `BrowserPod`'s logo and a link back to them, in the cell frame header.
///
/// A requirement of their free tier ("your application must display BrowserPod
/// attribution (visible link + logo)"), not a courtesy, which is why it sits
/// in the frame itself rather than in a footer someone might not scroll to. It
/// renders on Linux cells and nowhere else in ironpad.
///
/// The mark is vendored at `public/browserpod-mark.svg` and painted through a
/// CSS mask so it takes the page's own theme; their asset ships white-on-dark,
/// which would be invisible in light mode.
#[component]
fn LinuxAttribution() -> impl IntoView {
    view! {
        <a
            class="ironpad-linux-attribution"
            href="https://browserpod.io"
            target="_blank"
            rel="noopener noreferrer"
            title="Linux cells run on BrowserPod, by Leaning Technologies"
        >
            <span class="ironpad-linux-attribution-mark" aria-hidden="true"></span>
            <span class="ironpad-linux-attribution-name">"BrowserPod"</span>
        </a>
    }
}

// ── Pod execution ───────────────────────────────────────────────────────────

/// Hand the compiled program to the pod runtime and stream its output back.
///
/// Owns two JS closures for the duration of the run and no longer. They are
/// dropped when this future completes, and the runtime is written so that a
/// call after that can only ever be swallowed: its output sink wraps every
/// invocation in a `try`, because an exception thrown inside `BrowserPod`'s
/// `onOutput` wedges `run()` so it never resolves. A leaked closure would be
/// the alternative, and this codebase has paid for those before.
#[cfg(feature = "hydrate")]
async fn execute_in_pod(
    notebook_id: &str,
    cell_id: &str,
    wasm_blob: &[u8],
    output: RwSignal<String>,
    stage: RwSignal<Stage>,
    guard: RunGuard,
) -> Result<Outcome, String> {
    use wasm_bindgen::{closure::Closure, JsValue};

    ensure_runtime().await?;

    // Terminate during `Booting` cannot tear anything down until the runtime
    // script has executed, so the same epoch that covers the compile has to
    // cover this window too. Checked again after the key fetch below, since
    // that is another await and the last one before a machine exists.
    if !guard.may_boot() {
        return Ok(Outcome::cancelled());
    }

    // The key is deployment identity rather than a secret (a pod boots in the
    // browser with it), but it is still only fetched by a page that is about
    // to boot one, and only once.
    if !js::has_key() {
        match crate::server_fns::get_browserpod_key().await {
            Ok(Some(key)) => js::configure(&key),
            Ok(None) => {
                return Err(
                    "This ironpad instance has no BrowserPod key configured, so Linux \
                            cells cannot run here. Everything else in the notebook still works."
                        .to_string(),
                )
            }
            Err(e) => return Err(format!("Could not reach this instance for a run key: {e}")),
        }
    }

    if !guard.may_boot() {
        return Ok(Outcome::cancelled());
    }

    js::retain(notebook_id, cell_id);

    let on_text = Closure::<dyn FnMut(String)>::new(move |text: String| {
        // A write to a disposed signal is a no-op in Leptos; a read is what
        // panics. `try_set` keeps this true even if that ever changes.
        let _ = output.try_set(text);
    });
    let on_stage = Closure::<dyn FnMut(String)>::new(move |name: String| {
        let next = match name.as_str() {
            "booting" => Stage::Booting,
            "running" => Stage::Running,
            // Past the runtime's reporting horizon. Still a stage, not an
            // ending: the run is open, the stream is live, and Terminate has
            // to stay reachable.
            "unknown" => Stage::Unknown,
            _ => return,
        };
        let _ = stage.try_set(next);
    });

    let opts = js_sys::Object::new();
    let set = |key: &str, value: &JsValue| {
        let _ = js_sys::Reflect::set(&opts, &JsValue::from_str(key), value);
    };
    set("notebookId", &JsValue::from_str(notebook_id));
    set("cellId", &JsValue::from_str(cell_id));
    // `Uint8Array::from` copies into a fresh JS buffer. That matters: a VIEW
    // over wasm memory would hand the pod our entire linear memory as the
    // program's backing store.
    set("bytes", &js_sys::Uint8Array::from(wasm_blob));
    set("onText", on_text.as_ref());
    set("onStage", on_stage.as_ref());

    let promise =
        js::run(&opts).map_err(|e| format!("The Linux runtime refused the run: {e:?}"))?;
    let result = wasm_bindgen_futures::JsFuture::from(promise)
        .await
        .map_err(|e| pod_error_message(&e))?;

    let read = |key: &str| js_sys::Reflect::get(&result, &JsValue::from_str(key)).ok();
    Ok(Outcome {
        status: read("status")
            .and_then(|v| v.as_string())
            .unwrap_or_else(|| "finished".to_string()),
        #[allow(clippy::cast_possible_truncation)] // Exit statuses are 0-255.
        exit_code: read("exitCode")
            .and_then(|v| v.as_f64())
            .map(|c| c as i32),
        inferred: read("inferred").is_some_and(|v| v.as_bool().unwrap_or(false)),
    })
}

/// The reader-facing sentence out of a rejected pod promise.
///
/// The runtime rejects with an `Error` whose message is written to be read by
/// whoever clicked — an unreachable CDN, a missing key, a machine that would
/// not boot — so that message is preferred over the debug formatting.
#[cfg(feature = "hydrate")]
fn pod_error_message(err: &wasm_bindgen::JsValue) -> String {
    js_sys::Reflect::get(err, &wasm_bindgen::JsValue::from_str("message"))
        .ok()
        .and_then(|m| m.as_string())
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| format!("The Linux machine reported an error: {err:?}"))
}

#[cfg(test)]
mod tests {
    use super::{Outcome, RunGuard, Stage};
    use ironpad_common::{CellType, IronpadCell};

    fn cell(cell_type: CellType) -> IronpadCell {
        IronpadCell {
            id: "c1".to_string(),
            order: 0,
            label: "Cell".to_string(),
            cell_type,
            source: "fn main() {}".to_string(),
            cargo_toml: None,
            shared: false,
            collapsed: false,
            output_collapsed: false,
            version: 1,
            saved_output: None,
        }
    }

    fn outcome(status: &str, exit_code: Option<i32>, inferred: bool) -> Outcome {
        Outcome {
            status: status.to_string(),
            exit_code,
            inferred,
        }
    }

    #[test]
    fn a_clean_exit_reads_as_success() {
        let ok = outcome("exited", Some(0), false);
        assert_eq!(ok.note(), "exit 0");
        assert!(!ok.failed());
    }

    #[test]
    fn a_nonzero_exit_reads_as_failure() {
        let bad = outcome("exited", Some(101), false);
        assert_eq!(bad.note(), "exit 101");
        assert!(bad.failed());
    }

    #[test]
    fn an_inferred_finish_says_so_and_is_not_a_failure() {
        // The whole point of the distinction: with no sentinel there is no
        // exit status, and claiming success would be claiming evidence we do
        // not have.
        let guess = outcome("finished", None, true);
        assert_eq!(guess.note(), "finished (inferred)");
        assert!(!guess.failed());
    }

    #[test]
    fn an_unfinished_or_torn_down_run_is_not_a_failure() {
        // Neither has established anything about the program, so neither may
        // paint the run glyph red.
        assert!(!outcome("unknown", None, true).failed());
        assert!(!outcome("terminated", None, false).failed());
        assert_eq!(outcome("unknown", None, true).note(), "still running");
        assert_eq!(outcome("terminated", None, false).note(), "machine stopped");
    }

    #[test]
    fn a_linux_cell_is_never_runnable_so_nothing_can_autorun_it() {
        // THE never-autorun invariant (PRD-0066 T-008), pinned rather than
        // left to hold by construction.
        //
        // Nothing in this component checks "am I allowed to autorun": it
        // simply never reads the run queue. What keeps a pod from booting on
        // page load is that `is_runnable()` is false here, because the
        // public-notebook autorun, Run All and the reactive queue are all
        // built by filtering on it. Widening that predicate to include Linux
        // would silently turn every visit to a notebook into a metered pod
        // boot, and no other test in the tree would notice.
        assert!(
            !cell(CellType::Linux).is_runnable(),
            "a Linux cell must never enter a queue that runs without a click"
        );
        // The positive control: without it this test would pass against an
        // `is_runnable` that had been broken to return false for everything.
        assert!(cell(CellType::Code).is_runnable());
    }

    #[test]
    fn a_cell_starts_idle() {
        // Idle is the default because nothing may boot a pod before a click.
        assert_eq!(Stage::default(), Stage::Idle);
    }

    #[test]
    fn stopping_during_the_compile_is_not_a_run() {
        // Terminate before a machine exists ends the run with nothing having
        // happened, so the cell must not paint itself as having been run and
        // its empty terminal must not report on a program that never started.
        let stopped = outcome("cancelled", None, false);
        assert_eq!(stopped.note(), "stopped");
        assert!(!stopped.started());
        assert!(!stopped.failed());
        // Positive control: everything else did reach a machine.
        assert!(outcome("exited", Some(0), false).started());
        assert!(outcome("terminated", None, false).started());
    }

    #[test]
    fn the_reporting_horizon_keeps_the_cell_busy() {
        // THE invariant behind the "output keeps arriving; stopping it means
        // shutting the Linux machine down" notice. `busy` is what swaps Run
        // for Terminate, so a stage that is not busy has no Stop button — and
        // that notice would then be describing a control the reader cannot
        // see, about a stream the runtime had already stopped delivering.
        assert!(Stage::Unknown.busy(), "the run is still running");
        assert_eq!(Stage::Unknown.label(), "still running");

        for stage in [Stage::Compiling, Stage::Booting, Stage::Running] {
            assert!(stage.busy(), "{stage:?} is a run in flight");
        }
        // The other half: without these the test would pass against a `busy`
        // that had been broken to return true for everything, which would put
        // a Stop button on a cell nobody has ever clicked.
        assert!(!Stage::Idle.busy());
        assert!(!Stage::Done.busy());
        assert_eq!(Stage::Idle.label(), "");
        assert_eq!(Stage::Done.label(), "");
    }

    #[test]
    fn a_click_meaning_stop_never_produces_a_boot() {
        // The compile is the window this exists for: Terminate has no pod to
        // tear down there (the runtime script is not even loaded), so the only
        // way a Stop can be honoured is the spawned task refusing to boot when
        // it lands. Each of these three is a metered token.
        assert!(RunGuard::may_boot_at(Some(7), 7), "nothing has intervened");
        // Terminate, which bumps the epoch.
        assert!(!RunGuard::may_boot_at(Some(8), 7));
        // Disposed mid-compile: the reader navigated away, and there is nobody
        // left to read the output.
        assert!(!RunGuard::may_boot_at(None, 7));
        // Why an epoch rather than a flag: Stop and then Run again, while the
        // first compile is still in flight. A flag would have been cleared by
        // the second click and the first task would boot anyway.
        assert!(!RunGuard::may_boot_at(Some(9), 7));
    }
}
