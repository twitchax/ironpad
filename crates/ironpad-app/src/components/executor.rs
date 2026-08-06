// ── WASM executor bindings ──────────────────────────────────────────────────
//
// The JS-side executor bridge (`public/executor-bridge.js`) delegates WASM
// module loading and execution to a Web Worker.  These bindings provide a
// type-safe Rust API over the bridge for use from Leptos components.

// ── JS interop (client-side only) ───────────────────────────────────────────

#[cfg(feature = "hydrate")]
mod js {
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen]
    extern "C" {
        /// Load a compiled WASM blob for a cell.  Returns a `Promise<void>`.
        #[wasm_bindgen(js_namespace = IronpadExecutor, js_name = "loadBlob", catch)]
        pub fn load_blob(
            cell_id: &str,
            hash: &str,
            wasm_bytes: &js_sys::Uint8Array,
            js_glue: Option<String>,
        ) -> Result<js_sys::Promise, JsValue>;

        /// Execute a loaded cell with input bytes.  Returns a
        /// `Promise<{ outputBytes, displayText, typeTag }>`.
        ///
        /// Always async: wasm-bindgen cells may have async `cell_main`.
        #[wasm_bindgen(js_namespace = IronpadExecutor, catch)]
        pub fn execute(
            cell_id: &str,
            input_bytes: &js_sys::Uint8Array,
        ) -> Result<js_sys::Promise, JsValue>;

        /// Tick a simulation cell.  Returns a
        /// `Promise<{ width, height, rgbBytes, fallback? }>`.
        #[wasm_bindgen(js_namespace = IronpadExecutor, catch)]
        pub fn tick(cell_id: &str) -> Result<js_sys::Promise, JsValue>;

        /// Tick a LiveView cell.  Returns a
        /// `Promise<{ kind, content, fallback? }>`.
        #[wasm_bindgen(js_namespace = IronpadExecutor, js_name = "tickLive", catch)]
        pub fn tick_live(cell_id: &str) -> Result<js_sys::Promise, JsValue>;

        /// Terminate the running Web Worker, aborting any in-flight execution.
        /// A fresh Worker is automatically respawned by the bridge.
        #[wasm_bindgen(js_namespace = IronpadExecutor)]
        pub fn terminate();
    }
}

// ── Public API ──────────────────────────────────────────────────────────────

/// Compute a lightweight hash of a WASM blob for executor caching.
///
/// Uses FNV-1a (64-bit) to avoid pulling in a heavy hashing dependency on the
/// WASM client side.  The hash is only used to detect same-blob cache hits.
#[cfg(feature = "hydrate")]
pub fn hash_wasm_blob(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Terminate the executor's Web Worker, cancelling any running cell.
///
/// The bridge rejects pending Promises with `AbortError` and respawns a
/// fresh Worker.  Previously-loaded blobs must be re-loaded.
#[cfg(feature = "hydrate")]
pub fn terminate_executor() {
    js::terminate();
}

/// Load a compiled WASM blob into the executor's cache.
///
/// If a blob with the same `hash` is already loaded for the cell, this is a
/// no-op (cache hit).  The function is async because `WebAssembly.instantiate`
/// is async on the browser.
#[cfg(feature = "hydrate")]
pub async fn load_blob(
    cell_id: &str,
    hash: &str,
    bytes: &[u8],
    js_glue: Option<String>,
) -> Result<(), String> {
    let uint8 = js_sys::Uint8Array::from(bytes);
    let promise = js::load_blob(cell_id, hash, &uint8, js_glue).map_err(|e| format!("{e:?}"))?;

    wasm_bindgen_futures::JsFuture::from(promise)
        .await
        .map_err(|e| format!("{e:?}"))?;

    Ok(())
}

/// Fetch a URL and return the (ok-status) `Response`, mapping every JS-side
/// failure into a string error.
#[cfg(feature = "hydrate")]
async fn fetch_response(url: &str) -> Result<web_sys::Response, String> {
    use wasm_bindgen::JsCast as _;

    let window = web_sys::window().ok_or("no window")?;
    let resp = wasm_bindgen_futures::JsFuture::from(window.fetch_with_str(url))
        .await
        .map_err(|e| format!("fetch failed: {e:?}"))?;
    let resp: web_sys::Response = resp.dyn_into().map_err(|_| "not a Response".to_string())?;
    if !resp.ok() {
        return Err(format!("HTTP {} for {url}", resp.status()));
    }
    Ok(resp)
}

/// Fetch a snapshotted share blob (and its wasm-bindgen JS glue, when the
/// manifest says one exists) from the immutable `/share-blobs/` route
/// (PRD-0047). Returns `(wasm_bytes, js_glue)` ready for [`load_blob`].
#[cfg(feature = "hydrate")]
pub async fn fetch_share_blob(
    entry: &ironpad_common::ShareBlobEntry,
) -> Result<(Vec<u8>, Option<String>), String> {
    use wasm_bindgen_futures::JsFuture;

    let resp = fetch_response(&format!("/share-blobs/{}.wasm", entry.blob)).await?;
    let buf = JsFuture::from(resp.array_buffer().map_err(|e| format!("{e:?}"))?)
        .await
        .map_err(|e| format!("blob read failed: {e:?}"))?;
    let wasm_bytes = js_sys::Uint8Array::new(&buf).to_vec();

    let js_glue = if entry.has_js_glue {
        let resp = fetch_response(&format!("/share-blobs/{}.js", entry.blob)).await?;
        let text = JsFuture::from(resp.text().map_err(|e| format!("{e:?}"))?)
            .await
            .map_err(|e| format!("glue read failed: {e:?}"))?;
        Some(text.as_string().ok_or("glue not a string")?)
    } else {
        None
    };

    Ok((wasm_bytes, js_glue))
}

/// Execution result from running a cell: (`output_bytes`, `display_text`, `type_tag`, `ran_on_main_thread`).
#[cfg(feature = "hydrate")]
pub type CellExecResult = (Vec<u8>, Option<String>, Option<String>, bool);

/// Execute a previously-loaded cell with the given input bytes.
///
/// Returns a [`CellExecResult`]. The cell must have been loaded via
/// [`load_blob`] first; otherwise the executor throws.
///
/// Async because the JS executor always returns a Promise (wasm-bindgen cells
/// may have an async `cell_main`).
#[cfg(feature = "hydrate")]
pub async fn execute_cell(cell_id: &str, input_bytes: &[u8]) -> Result<CellExecResult, String> {
    let input = js_sys::Uint8Array::from(input_bytes);
    let promise = js::execute(cell_id, &input).map_err(|e| format!("{e:?}"))?;

    let result = wasm_bindgen_futures::JsFuture::from(promise)
        .await
        .map_err(|e| format!("{e:?}"))?;

    // Extract `outputBytes` (Uint8Array) from the result object.
    let output_val =
        js_sys::Reflect::get(&result, &"outputBytes".into()).map_err(|e| format!("{e:?}"))?;

    let output_bytes = if wasm_bindgen::JsCast::is_instance_of::<js_sys::Uint8Array>(&output_val) {
        js_sys::Uint8Array::from(output_val).to_vec()
    } else {
        vec![]
    };

    // Extract `displayText` (string | null).
    let display_val =
        js_sys::Reflect::get(&result, &"displayText".into()).map_err(|e| format!("{e:?}"))?;

    let display_text = if display_val.is_null() || display_val.is_undefined() {
        None
    } else {
        display_val.as_string()
    };

    // Extract `typeTag` (string | null).
    let type_tag_val =
        js_sys::Reflect::get(&result, &"typeTag".into()).map_err(|e| format!("{e:?}"))?;

    let type_tag = if type_tag_val.is_null() || type_tag_val.is_undefined() {
        None
    } else {
        type_tag_val.as_string()
    };

    // Extract `fallback` (bool) — true when execution fell back to the main thread.
    let fallback_val =
        js_sys::Reflect::get(&result, &"fallback".into()).map_err(|e| format!("{e:?}"))?;

    let ran_on_main_thread = fallback_val.as_bool().unwrap_or(false);

    Ok((output_bytes, display_text, type_tag, ran_on_main_thread))
}

// ── Tick (simulation cells) ─────────────────────────────────────────────────

/// Result of ticking a `LiveView` cell: content kind and string content.
pub struct LiveTickResult {
    pub kind: u32,
    pub content: String,
}

/// Result of ticking a simulation cell: frame dimensions and RGB pixel data.
pub struct TickResult {
    pub width: u32,
    pub height: u32,
    pub rgb_bytes: Vec<u8>,
}

/// Tick a simulation cell, returning one frame of pixel data.
///
/// The cell must have been loaded via [`load_blob`] and must export a
/// `cell_tick` function.  The JS executor calls `cell_tick` and returns the
/// resulting frame as `{ width, height, rgbBytes, fallback? }`.
#[cfg(feature = "hydrate")]
pub async fn tick_cell(cell_id: &str) -> Result<TickResult, String> {
    let promise = js::tick(cell_id).map_err(|e| format!("{e:?}"))?;

    let result = wasm_bindgen_futures::JsFuture::from(promise)
        .await
        .map_err(|e| format!("{e:?}"))?;

    // Extract `width` (number).
    let width_val = js_sys::Reflect::get(&result, &"width".into()).map_err(|e| format!("{e:?}"))?;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let width = width_val
        .as_f64()
        .ok_or_else(|| "tick result missing `width`".to_string())? as u32;

    // Extract `height` (number).
    let height_val =
        js_sys::Reflect::get(&result, &"height".into()).map_err(|e| format!("{e:?}"))?;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let height = height_val
        .as_f64()
        .ok_or_else(|| "tick result missing `height`".to_string())? as u32;

    // Extract `rgbBytes` (Uint8Array).
    let rgb_val =
        js_sys::Reflect::get(&result, &"rgbBytes".into()).map_err(|e| format!("{e:?}"))?;
    let rgb_bytes = js_sys::Uint8Array::new(&rgb_val).to_vec();

    Ok(TickResult {
        width,
        height,
        rgb_bytes,
    })
}

#[cfg(not(feature = "hydrate"))]
#[allow(clippy::unused_async)]
pub async fn tick_cell(_cell_id: &str) -> Result<TickResult, String> {
    Err("tick_cell is only available in hydrate mode".into())
}

// ── Tick (LiveView cells) ───────────────────────────────────────────────────

/// Tick a `LiveView` cell, returning the content string and kind.
///
/// The JS executor calls `cell_tick` and returns `{ kind, content, fallback? }`.
#[cfg(feature = "hydrate")]
pub async fn tick_live_cell(cell_id: &str) -> Result<LiveTickResult, String> {
    let promise = js::tick_live(cell_id).map_err(|e| format!("{e:?}"))?;
    let result = wasm_bindgen_futures::JsFuture::from(promise)
        .await
        .map_err(|e| format!("{e:?}"))?;

    let kind_val = js_sys::Reflect::get(&result, &"kind".into()).map_err(|e| format!("{e:?}"))?;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let kind = kind_val
        .as_f64()
        .ok_or_else(|| "tick_live result missing `kind`".to_string())? as u32;

    let content_val =
        js_sys::Reflect::get(&result, &"content".into()).map_err(|e| format!("{e:?}"))?;
    let content = content_val.as_string().unwrap_or_default();

    Ok(LiveTickResult { kind, content })
}

#[cfg(not(feature = "hydrate"))]
#[allow(clippy::unused_async)]
pub async fn tick_live_cell(_cell_id: &str) -> Result<LiveTickResult, String> {
    Err("tick_live_cell is only available in hydrate mode".into())
}

/// Encode upstream cell outputs in the `CellInputs` wire format:
/// `[count: u32 LE][len0: u32 LE][bytes0]...` — one entry per preceding
/// cell, empty for markdown/failed cells so indices stay positional.
///
/// The decoder counterpart is `CellInputs::from_raw` in `ironpad-cell`
/// (`input.rs`); the two must stay in sync.
#[allow(clippy::cast_possible_truncation)] // Cell counts/sizes fit u32 by construction.
pub fn encode_cell_inputs<T: AsRef<[u8]>>(outputs: &[T]) -> Vec<u8> {
    let total: usize = outputs.iter().map(|o| o.as_ref().len() + 4).sum();
    let mut buf = Vec::with_capacity(4 + total);
    buf.extend_from_slice(&(outputs.len() as u32).to_le_bytes());
    for output in outputs {
        let output = output.as_ref();
        buf.extend_from_slice(&(output.len() as u32).to_le_bytes());
        buf.extend_from_slice(output);
    }
    buf
}

/// The positional piping recipe: one slot per PRECEDING cell — a code
/// cell's latest output bytes + type tag, empty for markdown/unexecuted
/// cells (shared cells never execute, so they land empty via the missing
/// output) — returned as (encoded input buffer, `previous_cell_types`).
///
/// ONE definition on purpose: the editor and the read-only viewer must
/// derive identical `previous_cell_types` for the same notebook state,
/// because that vector feeds the blake3 cache key (`ironpad_common::
/// cache_key`) — a drift between two hand-maintained copies silently forks
/// cache identity between the editor and the viewer for the same cell. The
/// byte layout feeds `CellInputs::from_raw` in `ironpad-cell`.
pub fn assemble_cell_inputs<C: PipingCell, S: std::hash::BuildHasher>(
    cells: &[C],
    my_idx: usize,
    outputs: &std::collections::HashMap<
        String,
        crate::components::output_render::CellOutputData,
        S,
    >,
) -> (Vec<u8>, Vec<String>) {
    if my_idx == 0 {
        // Zero preceding cells: `CellInputs::from_raw` accepts the empty
        // buffer and the zero-count header alike; the empty buffer is free.
        return (Vec::new(), Vec::new());
    }
    let mut all_outputs: Vec<&[u8]> = Vec::with_capacity(my_idx);
    let mut types: Vec<String> = Vec::with_capacity(my_idx);
    for c in &cells[..my_idx] {
        if let Some(data) = c.is_code().then(|| outputs.get(c.id())).flatten() {
            all_outputs.push(&data.bytes);
            types.push(data.type_tag.clone().unwrap_or_default());
        } else {
            all_outputs.push(&[]);
            types.push(String::new());
        }
    }
    (encode_cell_inputs(&all_outputs), types)
}

/// The projection [`assemble_cell_inputs`] and [`unexecuted_dependencies`] need
/// — implemented for both cell shapes so the editor (`CellManifest`) and the
/// viewer (`IronpadCell`) run the SAME recipes instead of hand-synced
/// copies.
pub trait PipingCell {
    fn id(&self) -> &str;
    fn is_code(&self) -> bool;
    /// Run All / cascades execute this cell (code and not shared).
    fn is_runnable(&self) -> bool;
}

impl PipingCell for ironpad_common::CellManifest {
    fn id(&self) -> &str {
        &self.id
    }
    fn is_code(&self) -> bool {
        self.cell_type == ironpad_common::CellType::Code
    }
    fn is_runnable(&self) -> bool {
        Self::is_runnable(self)
    }
}

impl PipingCell for ironpad_common::IronpadCell {
    fn id(&self) -> &str {
        &self.id
    }
    fn is_code(&self) -> bool {
        self.cell_type == ironpad_common::CellType::Code
    }
    fn is_runnable(&self) -> bool {
        Self::is_runnable(self)
    }
}

/// Project `cells` + a source lookup into the dependency graph's shape.
fn dep_cells<'a, C: PipingCell>(
    cells: &'a [C],
    sources: &'a [Option<String>],
) -> Vec<ironpad_common::cell_deps::DepCell<'a>> {
    cells
        .iter()
        .zip(sources)
        .map(|(c, s)| ironpad_common::cell_deps::DepCell {
            runnable: c.is_runnable(),
            source: s.as_deref(),
        })
        .collect()
}

/// The dependency-aware cascade recipe (PRD-0060): the unexecuted subset of
/// `cell_id`'s transitive dependency closure, in notebook order — the
/// prerequisites a single-cell Run must execute first, or the target's piped
/// inputs arrive empty and it errors out on content the author already made
/// work. Cells the target never consumes are NOT cascaded; the scaffold
/// binds only referenced slots, so skipping them is safe.
///
/// `source_of` supplies each cell's current text; `None` degrades that cell
/// to depends-on-all-upstream (the pre-0060 behavior). The same cascade
/// semantics `run_all_queue` gives agents (PRD-0052), shared with the
/// read-only viewer's Run button.
pub fn unexecuted_dependencies<C: PipingCell, S: std::hash::BuildHasher>(
    cells: &[C],
    cell_id: &str,
    outputs: &std::collections::HashMap<
        String,
        crate::components::output_render::CellOutputData,
        S,
    >,
    source_of: impl Fn(&str) -> Option<String>,
) -> Vec<String> {
    let Some(my_idx) = cells.iter().position(|c| c.id() == cell_id) else {
        return Vec::new();
    };
    let sources: Vec<Option<String>> = cells.iter().map(|c| source_of(c.id())).collect();
    let graph = dep_cells(cells, &sources);
    ironpad_common::cell_deps::transitive_dep_indices(&graph, my_idx)
        .into_iter()
        .filter(|&i| !outputs.contains_key(cells[i].id()))
        .map(|i| cells[i].id().to_string())
        .collect()
}

/// Cells in `queue` whose transitive dependencies include `failed_id` — the
/// ones that cannot produce an honest result after that failure and must be
/// dropped (and surfaced as blocked). Everything else in the queue keeps
/// running: the continue-past-failures policy (PRD-0060), which is what
/// lets a notebook opening with a deliberate compile-fail teaching cell
/// still run everything independent of it.
pub fn dependents_in_queue<C: PipingCell>(
    cells: &[C],
    queue: &[String],
    failed_id: &str,
    source_of: impl Fn(&str) -> Option<String>,
) -> Vec<String> {
    let Some(failed_idx) = cells.iter().position(|c| c.id() == failed_id) else {
        return Vec::new();
    };
    let sources: Vec<Option<String>> = cells.iter().map(|c| source_of(c.id())).collect();
    let graph = dep_cells(cells, &sources);
    queue
        .iter()
        .filter(|qid| qid.as_str() != failed_id)
        .filter(|qid| {
            cells
                .iter()
                .position(|c| c.id() == qid.as_str())
                .is_some_and(|qi| {
                    ironpad_common::cell_deps::transitive_dep_indices(&graph, qi)
                        .contains(&failed_idx)
                })
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        assemble_cell_inputs, dependents_in_queue, encode_cell_inputs, unexecuted_dependencies,
    };
    use crate::components::output_render::CellOutputData;
    use ironpad_common::{CellManifest, CellType};
    use std::collections::HashMap;

    fn manifest(id: &str, cell_type: CellType) -> CellManifest {
        CellManifest {
            id: id.to_string(),
            order: 0,
            label: id.to_string(),
            cell_type,
            shared: false,
            collapsed: false,
            output_collapsed: false,
        }
    }

    #[test]
    fn assembles_positional_slots_with_empty_markdown_and_unexecuted() {
        let cells = vec![
            manifest("c0", CellType::Code),
            manifest("md", CellType::Markdown),
            manifest("c1", CellType::Code),
            manifest("me", CellType::Code),
        ];
        let mut outputs = HashMap::new();
        outputs.insert(
            "c0".to_string(),
            CellOutputData {
                bytes: vec![1, 2],
                type_tag: Some("u32".to_string()),
            },
        );
        // c1 exists but never executed: empty slot, empty tag.
        let (buf, types) = assemble_cell_inputs(&cells, 3, &outputs);
        assert_eq!(types, vec!["u32".to_string(), String::new(), String::new()]);
        assert_eq!(buf, encode_cell_inputs(&[&[1u8, 2][..], &[], &[]]));
    }

    #[test]
    fn first_cell_gets_the_free_empty_encoding() {
        let cells = vec![manifest("me", CellType::Code)];
        let (buf, types) = assemble_cell_inputs(&cells, 0, &HashMap::new());
        assert!(buf.is_empty());
        assert!(types.is_empty());
    }

    #[test]
    fn unexecuted_dependencies_cascade_only_consumed_cells() {
        let mut cells = vec![
            manifest("ran", CellType::Code),    // slot 0, executed
            manifest("md", CellType::Markdown), // slot 1
            manifest("cold", CellType::Code),   // slot 2, unexecuted
            manifest("shared", CellType::Code), // slot 3, shared
            manifest("me", CellType::Code),     // slot 4
            manifest("after", CellType::Code),  // slot 5
        ];
        cells[3].shared = true;
        let mut outputs = HashMap::new();
        outputs.insert("ran".to_string(), CellOutputData::default());

        // The target consumes slot 2 only: the unexecuted dependency
        // cascades, and nothing else does.
        let sources: HashMap<&str, &str> = [
            ("ran", "1"),
            ("cold", "2"),
            ("me", "cell2 + 1"),
            ("after", "3"),
        ]
        .into();
        let source_of = |id: &str| sources.get(id).map(ToString::to_string);
        assert_eq!(
            unexecuted_dependencies(&cells, "me", &outputs, source_of),
            vec!["cold".to_string()]
        );

        // A target consuming nothing cascades nothing — even with cold
        // upstream cells present.
        let independent = |id: &str| {
            if id == "me" {
                Some("40 + 2".to_string())
            } else {
                sources.get(id).map(ToString::to_string)
            }
        };
        assert_eq!(
            unexecuted_dependencies(&cells, "me", &outputs, independent),
            Vec::<String>::new()
        );

        // An executed dependency is warm: consuming slot 0 cascades nothing.
        let warm = |id: &str| {
            if id == "me" {
                Some("cell0".to_string())
            } else {
                sources.get(id).map(ToString::to_string)
            }
        };
        assert_eq!(
            unexecuted_dependencies(&cells, "me", &outputs, warm),
            Vec::<String>::new()
        );

        // Missing source degrades to depends-on-all-upstream (pre-0060).
        assert_eq!(
            unexecuted_dependencies(&cells, "me", &outputs, |_| None),
            vec!["cold".to_string()]
        );

        // Unknown target: nothing (never "run everything").
        assert_eq!(
            unexecuted_dependencies(&cells, "gone", &outputs, source_of),
            Vec::<String>::new()
        );
    }

    #[test]
    fn dependents_in_queue_partitions_by_transitive_dependency() {
        let cells = vec![
            manifest("fail", CellType::Code),  // slot 0
            manifest("dep", CellType::Code),   // slot 1: consumes slot 0
            manifest("indep", CellType::Code), // slot 2: independent
            manifest("chain", CellType::Code), // slot 3: consumes slot 1 (transitively slot 0)
        ];
        let sources: HashMap<&str, &str> = [
            ("fail", "compile error here"),
            ("dep", "cell0 * 2"),
            ("indep", "42"),
            ("chain", "cell1 + 1"),
        ]
        .into();
        let source_of = |id: &str| sources.get(id).map(ToString::to_string);

        let queue: Vec<String> = ["fail", "dep", "indep", "chain"]
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(
            dependents_in_queue(&cells, &queue, "fail", source_of),
            vec!["dep".to_string(), "chain".to_string()],
            "direct and transitive dependents drop; the independent cell survives"
        );

        // The failed cell itself is never listed (it is popped, not blocked).
        assert!(
            !dependents_in_queue(&cells, &queue, "fail", source_of).contains(&"fail".to_string())
        );

        // Unknown failed id: nothing to drop.
        assert_eq!(
            dependents_in_queue(&cells, &queue, "gone", source_of),
            Vec::<String>::new()
        );
    }

    #[test]
    fn encodes_length_prefixed_entries() {
        let buf = encode_cell_inputs(&[b"ab".as_slice(), b"".as_slice(), b"xyz".as_slice()]);
        let mut expect = Vec::new();
        expect.extend_from_slice(&3u32.to_le_bytes());
        expect.extend_from_slice(&2u32.to_le_bytes());
        expect.extend_from_slice(b"ab");
        expect.extend_from_slice(&0u32.to_le_bytes());
        expect.extend_from_slice(&3u32.to_le_bytes());
        expect.extend_from_slice(b"xyz");
        assert_eq!(buf, expect);
    }

    #[test]
    fn encodes_empty_input_list() {
        assert_eq!(encode_cell_inputs::<&[u8]>(&[]), 0u32.to_le_bytes());
    }
}
