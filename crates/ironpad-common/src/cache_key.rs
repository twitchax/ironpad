//! The compilation cache-key recipe, shared by server and browser.
//!
//! Everything that feeds a cell's blake3 content hash lives here — the hash
//! itself, the epoch/target constants, and the pure feature-detection
//! functions (rayon/atomics, autodiff, SIMD) whose booleans are part of the
//! key. The server (`ironpad-app/src/compiler/cache.rs`) delegates to
//! [`content_hash_with_fingerprint`] with its process-cached toolchain
//! fingerprint; the browser calls the same function with a fingerprint
//! fetched once per session (PRD-0047), so both sides compute identical keys
//! from one recipe instead of two drifting reimplementations.

/// The compilation target baked into the cache key so that a future target
/// change automatically invalidates existing entries.
const TARGET_TRIPLE: &str = "wasm32-unknown-unknown";

/// Monotonic epoch counter baked into the cache key.
///
/// Bump this whenever the compilation pipeline changes in a way that should
/// invalidate all cached blobs (e.g. RUSTFLAGS changes, wasm-bindgen
/// post-processing changes, scaffold template changes, `ironpad-cell` source
/// edits — there is no automatic hash of the injected `ironpad-cell` crate's
/// contents, so bump this manually when it changes).
///
/// Bumped 1 -> 2 for PRD-0031 T-003: folding the toolchain fingerprint into
/// the key should invalidate all pre-existing blobs once, since their
/// toolchain provenance is unknown.
///
/// Caveat: the fingerprint tracks only `CELL_TOOLCHAIN`, so bumping
/// `CELL_TOOLCHAIN` invalidates every blob automatically, but bumping the
/// split-out pins (`AUTODIFF_TOOLCHAIN` or `ATOMICS_TOOLCHAIN` in
/// `compiler/build.rs`) does NOT — bump this epoch when you change one of those
/// so their cells rebuild against the new toolchain.
///
/// Bumped 2 -> 3 for PRD-0036 T-008: `ironpad-cell`'s `CellInputs::from_raw`
/// gained bounds checking, so cached cells should rebuild against the safer
/// runtime.
///
/// Bumped 3 -> 4 for PRD-0038 T-001: variable-length fields are now
/// length-prefixed (framed) in the hash input, which changes every key, so
/// pre-existing blobs (hashed with the old bare-concatenation scheme) must be
/// invalidated once.
///
/// Bumped 4 -> 5 for PRD-0043 T-001: `ironpad-cell` gained the `blocking`
/// module (JSPI host imports), so cached cells must rebuild against the new
/// runtime. (The `needs_simd` hash byte added in the same release also
/// changes every key; the bump keeps the documented ironpad-cell discipline.)
///
/// Bumped 5 -> 6: `IntoPanels for Canvas` switched from an Html panel (whose
/// data: URI the sanitizer strips — tuple outputs rendered an empty image) to
/// the structured `BlobImage` panel; cached blobs bake the old behavior in.
///
/// Bumped 6 -> 7: the scaffold now emits `#[allow(dead_code)] mod shared;`, so
/// shared helpers a cell does not call no longer report the false "never used"
/// warning. The scaffold is not part of the hash input, and the cache stores
/// DIAGNOSTICS next to the blob, so without this bump every already-cached cell
/// would keep replaying the stale warnings forever.
///
/// Bumped 7 -> 8 (two scaffold diagnostic fixes in one release): the injected
/// `cellN` / `last` input bindings now carry `#[allow(unused_variables)]`, so
/// cells that use only some (or none) of their upstream outputs no longer
/// warn; and the sim/live-view tick wrappers reach their `static mut` through
/// a raw pointer, silencing the `static_mut_refs` warning that mapped past
/// the end of the user's source. Same rationale as 6 -> 7: cached diagnostics
/// would replay the stale warnings without the bump.
///
/// Bumped 8 -> 9 (PRD-0060, three pipeline changes in one release): cells
/// compile on edition 2024 (`gen` blocks are 2024-only syntax, so the
/// `gen_blocks` gate was inert on 2021); `cell_main` became a one-line
/// trampoline into a plain inner fn so `#[wasm_bindgen]`'s syn parser never
/// sees user tokens (syn rejects `gen { … }` even on 2024); and the
/// scaffold binds only the `cellN` slots the source references — with
/// `previous_cell_types` normalized to match inside this hash, so
/// unreferenced upstream types no longer fork the key.
///
/// Bumped 9 -> 10 (all-up review): the `last`-alias matcher excluded any
/// `last` preceded by `.`, which also swallowed the `..` range / struct
/// update operator (`..last.clone()`, `0..last.len()`), so a cell whose only
/// alias use was in `..last` form under-cascaded and omitted the binding.
/// Fixing detection changes which cells depend-on-all, so cached blobs keyed
/// on the old verdict must invalidate.
pub const CACHE_EPOCH: u32 = 10;

// ── Content hash ─────────────────────────────────────────────────────────────

/// Compute the deterministic blake3 cache key for a cell, given an explicit
/// toolchain fingerprint.
///
/// The hash includes the fixed target triple so any future target change
/// naturally invalidates the cache. Every variable-length field is
/// length-prefixed (framed) so field boundaries are unambiguous — otherwise
/// distinct inputs whose bytes concatenate identically would collide (e.g.
/// `("ab", "c")` and `("a", "bc")`) and serve each other's compiled WASM.
/// The toolchain fingerprint (rustc version + host wasm-bindgen CLI version)
/// ensures a toolchain upgrade invalidates stale cached blobs.
#[allow(clippy::too_many_arguments)]
#[allow(clippy::fn_params_excessive_bools)]
#[must_use]
pub fn content_hash_with_fingerprint(
    source: &str,
    cargo_toml: &str,
    previous_types: &[String],
    shared_cargo_toml: Option<&str>,
    shared_source: Option<&str>,
    needs_atomics: bool,
    needs_autodiff: bool,
    needs_simd: bool,
    toolchain: &str,
) -> String {
    // Every variable-length field is length-prefixed so its boundary with the
    // next field is unambiguous. A bare concatenation (`source || cargo_toml ||
    // …`) lets distinct inputs whose bytes line up collide and serve each
    // other's cached WASM — e.g. `("ab", "c")` vs `("a", "bc")`, or a
    // `cargo_toml` ending in the target triple bytes vs a shorter one.
    // Only the types the cell can actually consume feed the key (PRD-0060):
    // the scaffold binds exactly the referenced slots, so an unreferenced
    // upstream type tag cannot change the generated code and must not
    // change the key. Normalizing HERE (not at call sites) keeps the
    // server, the browser blob cache, and share-snapshot recomputes on one
    // recipe by construction.
    let previous_types = crate::cell_deps::normalize_previous_types(source, previous_types);
    let mut hasher = blake3::Hasher::new();
    update_framed(&mut hasher, source.as_bytes());
    update_framed(&mut hasher, cargo_toml.as_bytes());
    update_framed(&mut hasher, TARGET_TRIPLE.as_bytes());
    update_framed(&mut hasher, &(previous_types.len() as u64).to_le_bytes());
    for t in &previous_types {
        update_framed(&mut hasher, t.as_bytes());
    }
    update_framed_opt(&mut hasher, shared_cargo_toml.map(str::as_bytes));
    update_framed_opt(&mut hasher, shared_source.map(str::as_bytes));
    hasher.update(&[u8::from(needs_atomics)]);
    hasher.update(&[u8::from(needs_autodiff)]);
    hasher.update(&[u8::from(needs_simd)]);
    hasher.update(&CACHE_EPOCH.to_le_bytes());
    update_framed(&mut hasher, toolchain.as_bytes());
    hasher.finalize().to_hex().to_string()
}

/// Feed `bytes` into the hasher framed by an 8-byte little-endian length
/// prefix, so the boundary between this field and the next is unambiguous and
/// no two distinct field splits can produce the same byte stream.
fn update_framed(hasher: &mut blake3::Hasher, bytes: &[u8]) {
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

/// Frame an optional field: a presence byte (`1`/`0`) followed by the framed
/// content when present. Keeps `None` distinct from `Some("")`.
fn update_framed_opt(hasher: &mut blake3::Hasher, bytes: Option<&[u8]>) {
    match bytes {
        Some(b) => {
            hasher.update(&[1]);
            update_framed(hasher, b);
        }
        None => {
            hasher.update(&[0]);
        }
    }
}

// ── Dependency merging ───────────────────────────────────────────────────────

/// The two shapes a dependency takes under `[dependencies]`: an inline
/// `name = spec` line, or a dotted `[dependencies.name]` subtable (kept as a
/// rendered block, header included).
#[derive(Debug, Clone)]
enum DepEntry {
    Inline(String),
    Section(String),
}

/// `[dependencies.NAME]` -> `NAME` (single-segment only).
fn dotted_dependency_name(header: &str) -> Option<&str> {
    let inner = header.strip_prefix('[')?.strip_suffix(']')?.trim();
    let name = inner.strip_prefix("dependencies.")?.trim();
    (!name.is_empty() && !name.contains('.')).then_some(name)
}

/// Parse a `Cargo.toml`'s dependency surface into (normalized crate name,
/// entry) pairs in declaration order. Dotted `[dependencies.NAME]` subtables
/// count: they used to be dropped as "some other section", which silently
/// bypassed the shared/cell merge, the rayon/atomics detection, and the
/// `ironpad-cell` filter for any notebook using the section form.
fn parse_dependency_entries(cargo_toml: &str) -> Vec<(String, DepEntry)> {
    enum State {
        Outside,
        Inline,
        Section,
    }
    let mut state = State::Outside;
    let mut entries: Vec<(String, DepEntry)> = Vec::new();

    for line in cargo_toml.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            if trimmed == "[dependencies]" {
                state = State::Inline;
            } else if let Some(name) = dotted_dependency_name(trimmed) {
                entries.push((name.replace('-', "_"), DepEntry::Section(line.to_string())));
                state = State::Section;
            } else {
                state = State::Outside;
            }
            continue;
        }
        match state {
            State::Inline => {
                if !trimmed.is_empty() && !trimmed.starts_with('#') {
                    if let Some(name) = crate_name_from_dep_line(line) {
                        entries.push((name, DepEntry::Inline(line.to_string())));
                    }
                }
            }
            State::Section => {
                if !trimmed.is_empty() {
                    if let Some((_, DepEntry::Section(block))) = entries.last_mut() {
                        block.push('\n');
                        block.push_str(line);
                    }
                }
            }
            State::Outside => {}
        }
    }
    entries
}

/// [`parse_dependency_entries`] minus any user-specified `ironpad-cell`
/// (we always inject our own), in either form.
fn user_dependency_entries(cargo_toml: &str) -> Vec<(String, DepEntry)> {
    parse_dependency_entries(cargo_toml)
        .into_iter()
        .filter(|(name, _)| name != "ironpad_cell")
        .collect()
}

/// Render entries: inline lines first, dotted subtables after — the order
/// TOML requires when the result is pasted under a `[dependencies]` header.
fn render_dep_entries(entries: Vec<(String, DepEntry)>) -> String {
    let mut inline = Vec::new();
    let mut sections = Vec::new();
    for (_, entry) in entries {
        match entry {
            DepEntry::Inline(line) => inline.push(line),
            DepEntry::Section(block) => sections.push(block),
        }
    }
    let mut out = inline.join("\n");
    for block in sections {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(&block);
    }
    out
}

/// Extract dependency declarations from the user's `Cargo.toml` content —
/// inline lines under `[dependencies]` AND dotted `[dependencies.NAME]`
/// subtables — filtering out any existing `ironpad-cell` entry (we always
/// inject our own).
#[must_use]
pub fn extract_user_dependencies(cargo_toml: &str) -> String {
    render_dep_entries(user_dependency_entries(cargo_toml))
}

/// True when the manifest declares any dependency beyond `ironpad-cell`, in
/// either the inline or dotted form. The single source for "does this cell
/// pull in a custom dep" — the scaffold merge, and the live-check warmth
/// gate ([`crate::types::manifest_has_custom_deps`]) both read it, so a new
/// dependency shape can never be understood by one and missed by the other.
#[must_use]
pub fn has_user_dependencies(cargo_toml: &str) -> bool {
    !user_dependency_entries(cargo_toml).is_empty()
}

/// True when `header` is a single-segment `[dependencies.NAME]` subtable —
/// exactly the form [`merge_dependencies`] captures and renders back out
/// under `[dependencies]`. The scaffold's extra-section collector must skip
/// these, or the subtable is emitted twice and cargo rejects the manifest
/// with `duplicate key`.
#[must_use]
pub fn is_dotted_dependency_section(header: &str) -> bool {
    dotted_dependency_name(header.trim()).is_some()
}

/// Merge shared (notebook-level) and cell-level dependencies.
///
/// Cell deps take precedence by normalized crate name, ACROSS forms: a cell
/// inline `rayon = "1"` replaces a shared `[dependencies.rayon]` subtable
/// and vice versa.
#[must_use]
pub fn merge_dependencies(shared_cargo_toml: Option<&str>, cell_cargo_toml: &str) -> String {
    let mut merged = shared_cargo_toml.map_or_else(Vec::new, user_dependency_entries);
    for (name, entry) in user_dependency_entries(cell_cargo_toml) {
        if let Some(existing) = merged.iter_mut().find(|(n, _)| *n == name) {
            existing.1 = entry;
        } else {
            merged.push((name, entry));
        }
    }
    render_dep_entries(merged)
}

/// Extract the crate name from a TOML dependency line.
///
/// Handles both `crate = "version"` and `crate = { ... }` forms.
/// Normalizes hyphens to underscores for comparison.
#[must_use]
pub fn crate_name_from_dep_line(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let name = trimmed.split('=').next()?.trim();
    if name.is_empty() {
        return None;
    }
    Some(name.replace('-', "_"))
}

// ── Feature detection ────────────────────────────────────────────────────────

/// Returns `true` if the merged dependency set (shared + cell) contains `rayon`.
///
/// Used to set the `needs_atomics` flag on the compilation pipeline so
/// downstream stages can enable the atomics WASM feature.
#[must_use]
pub fn merged_deps_contain_rayon(shared_cargo_toml: Option<&str>, cell_cargo_toml: &str) -> bool {
    // Entry names, not rendered lines: a `[dependencies.rayon]` subtable
    // has no `rayon = ...` line for a line-based scan to find.
    shared_cargo_toml
        .map_or_else(Vec::new, user_dependency_entries)
        .iter()
        .chain(user_dependency_entries(cell_cargo_toml).iter())
        .any(|(name, _)| name == "rayon")
}

/// Returns `true` if the cell (or the notebook's shared source) uses
/// `std::autodiff`, opting the build into the Enzyme pipeline: crate-root
/// feature gate, fat-LTO profile, nightly toolchain, and
/// `-Zautodiff=Enable` (PRD-0041).
///
/// Substring detection mirrors the rayon opt-in's spirit: using the feature
/// IS the opt-in. False positives (e.g. the strings in a comment) cost only a
/// slower compile profile, never a wrong result.
#[must_use]
pub fn uses_std_autodiff(source: &str, shared_source: Option<&str>) -> bool {
    let hit = |s: &str| {
        s.contains("autodiff_forward")
            || s.contains("autodiff_reverse")
            || s.contains("std::autodiff")
    };
    hit(source) || shared_source.is_some_and(hit)
}

/// Returns `true` if the cell (or the notebook's shared source) uses WASM
/// SIMD, opting the build into `-C target-feature=+simd128` and a crate-root
/// `#![feature(portable_simd)]` gate (PRD-0042).
///
/// Same substring-detection spirit as [`uses_std_autodiff`]: using the feature
/// IS the opt-in. A false positive (the strings in a comment) costs only an
/// unused feature gate and a harmless codegen flag — every current browser
/// instantiates simd128 modules natively.
#[must_use]
pub fn uses_wasm_simd(source: &str, shared_source: Option<&str>) -> bool {
    let hit = |s: &str| {
        s.contains("std::simd") || s.contains("core::simd") || s.contains("std::arch::wasm32")
    };
    hit(source) || shared_source.is_some_and(hit)
}

/// Returns `true` if the cell (or the notebook's shared source) uses
/// coroutines, opting the build into the crate-root
/// `#![feature(coroutines, coroutine_trait, stmt_expr_attributes)]` gate.
///
/// `stmt_expr_attributes` rides along because `#[coroutine]` attaches to a
/// closure *expression*, which is an attribute-on-expression position.
///
/// Same substring-detection spirit as [`uses_wasm_simd`]: using the feature IS
/// the opt-in, and a false positive costs an unused feature gate, which is
/// inert. Deliberately narrow (no bare `yield`, which appears in ordinary
/// prose and string literals).
#[must_use]
pub fn uses_coroutines(source: &str, shared_source: Option<&str>) -> bool {
    let hit = |s: &str| {
        s.contains("#[coroutine]") || s.contains("CoroutineState") || s.contains("ops::Coroutine")
    };
    hit(source) || shared_source.is_some_and(hit)
}

/// Returns `true` if the cell (or the shared source) uses `gen` blocks —
/// the second `yield` surface, which needs its own crate-root gate
/// (`#![feature(gen_blocks)]`) since it carries none of the coroutine
/// markers. This closes the documented gap in the coroutine detection.
///
/// A bare `gen` is ordinary English, so detection matches the block forms
/// only: a word-boundary `gen`, optionally followed by `move`, then a `{`
/// after any whitespace (`gen{`, `gen  {`, `gen\n{`, `gen move{` are all
/// valid token streams on edition 2024) — plus `async gen` as its own
/// marker. A false positive (the string in a comment) costs one harmless
/// feature gate; a miss dead-ends the author, because rustc's E0658 help
/// names a crate-root attribute a cell cannot reach (the crate root belongs
/// to the scaffold) — hence the whitespace-tolerant scan.
#[must_use]
pub fn uses_gen_blocks(source: &str, shared_source: Option<&str>) -> bool {
    fn is_ident(b: u8) -> bool {
        b.is_ascii_alphanumeric() || b == b'_'
    }
    fn hit(s: &str) -> bool {
        let bytes = s.as_bytes();
        let mut from = 0;
        while let Some(pos) = s[from..].find("gen").map(|p| p + from) {
            from = pos + 1;
            let end = pos + 3;
            let word_boundary = (pos == 0 || !is_ident(bytes[pos - 1]))
                && (end >= bytes.len() || !is_ident(bytes[end]));
            if !word_boundary {
                continue;
            }
            // `async gen` is a marker on its own (async gen blocks).
            let before = s[..pos].trim_end();
            if before.ends_with("async")
                && (before.len() == 5 || !is_ident(before.as_bytes()[before.len() - 6]))
            {
                return true;
            }
            // `gen [move] {` with any whitespace (including none).
            let mut rest = s[end..].trim_start();
            if let Some(after_move) = rest.strip_prefix("move") {
                if after_move.starts_with(|c: char| c.is_whitespace() || c == '{') {
                    rest = after_move.trim_start();
                }
            }
            if rest.starts_with('{') {
                return true;
            }
        }
        false
    }
    hit(source) || shared_source.is_some_and(hit)
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dotted_dependency_sections_are_extracted_merged_and_filtered() {
        // Regression: `[dependencies.NAME]` subtables were treated as "some
        // other section" and dropped wholesale.
        let toml = "[package]\nname = \"x\"\n\n[dependencies]\nserde = \"1\"\n\n\
                    [dependencies.rayon]\nversion = \"1.10\"\n\n\
                    [dependencies.ironpad-cell]\npath = \"nope\"\n";
        let extracted = extract_user_dependencies(toml);
        assert_eq!(
            extracted, "serde = \"1\"\n\n[dependencies.rayon]\nversion = \"1.10\"",
            "inline first, subtable kept with its header, ironpad-cell filtered"
        );

        // Cross-form precedence: a cell INLINE line replaces a shared
        // SUBTABLE of the same crate, and vice versa.
        let shared = "[dependencies.rayon]\nversion = \"1.9\"\n";
        let merged = merge_dependencies(Some(shared), "[dependencies]\nrayon = \"1.10\"\n");
        assert_eq!(merged, "rayon = \"1.10\"");
        let merged = merge_dependencies(
            Some("[dependencies]\nrayon = \"1.9\"\n"),
            "[dependencies.rayon]\nversion = \"1.10\"\n",
        );
        assert_eq!(merged, "[dependencies.rayon]\nversion = \"1.10\"");
    }

    #[test]
    fn rayon_in_a_dotted_section_opts_into_atomics() {
        // The detection feeds the atomics toolchain/flags AND the cache key;
        // the section form used to be invisible to it.
        assert!(merged_deps_contain_rayon(
            None,
            "[dependencies.rayon]\nversion = \"1.10\"\n"
        ));
        assert!(merged_deps_contain_rayon(
            Some("[dependencies.rayon]\nversion = \"1.10\"\n"),
            "",
        ));
        assert!(!merged_deps_contain_rayon(
            None,
            "[dependencies.serde]\nversion = \"1\"\n"
        ));
        // The inline form still detects.
        assert!(merged_deps_contain_rayon(
            None,
            "[dependencies]\nrayon = \"1.10\"\n"
        ));
    }

    #[test]
    fn gen_block_detection_matches_block_forms_not_bare_gen() {
        assert!(uses_gen_blocks("let it = gen { yield 1; };", None));
        assert!(uses_gen_blocks("let it = gen move { yield x; };", None));
        assert!(uses_gen_blocks("let s = async gen { yield 1; };", None));
        // Shared source counts too.
        assert!(uses_gen_blocks(
            "42",
            Some("pub fn f() { gen { yield 1; }; }")
        ));
        // Whitespace variants are the same valid token stream on edition
        // 2024 — a miss here dead-ends the author on an E0658 naming a
        // crate-root gate a cell cannot reach.
        assert!(uses_gen_blocks("let g = gen{ yield 1u32; };", None));
        assert!(uses_gen_blocks("let g = gen  { yield 1; };", None));
        assert!(uses_gen_blocks("let g = gen\n{ yield 1; };", None));
        assert!(uses_gen_blocks("let g = gen move{ yield x; };", None));
        assert!(uses_gen_blocks("let s = async\ngen { yield 1; };", None));
        // Bare "gen" is ordinary prose/identifier text, never an opt-in.
        assert!(!uses_gen_blocks("// generate the gen next", None));
        assert!(!uses_gen_blocks("let gen_count = 3;", None));
        assert!(!uses_gen_blocks("degen { }", None));
        assert!(!uses_gen_blocks("let masync = 1; masync gen", None));
        assert!(!uses_gen_blocks("gen movee { }", None));
    }

    #[test]
    fn coroutine_detection_covers_source_and_shared_but_not_bare_yield() {
        assert!(uses_coroutines(
            "let c = #[coroutine] |_: ()| { yield 1; };",
            None
        ));
        assert!(uses_coroutines(
            "match c.resume(()) { CoroutineState::Yielded(v) => v }",
            None
        ));
        assert!(uses_coroutines("use std::ops::Coroutine;", None));
        assert!(uses_coroutines(
            "shared::drive()",
            Some("use std::ops::Coroutine;")
        ));
        // Bare "yield" is ordinary English and must not opt a cell in.
        assert!(!uses_coroutines(
            "the pipeline will yield a better result",
            None
        ));
        assert!(!uses_coroutines("let x = 1;", None));
        assert!(!uses_coroutines("let x = 1;", Some("pub fn f() {}")));
    }

    #[test]
    fn hash_changes_when_toolchain_fingerprint_changes() {
        let a = content_hash_with_fingerprint(
            "x",
            "y",
            &[],
            None,
            None,
            false,
            false,
            false,
            "toolchain-a",
        );
        let b = content_hash_with_fingerprint(
            "x",
            "y",
            &[],
            None,
            None,
            false,
            false,
            false,
            "toolchain-b",
        );
        assert_ne!(a, b);
    }

    #[test]
    fn hash_is_deterministic_for_same_toolchain() {
        let a = content_hash_with_fingerprint(
            "x",
            "y",
            &[],
            None,
            None,
            false,
            false,
            false,
            "toolchain-a",
        );
        let b = content_hash_with_fingerprint(
            "x",
            "y",
            &[],
            None,
            None,
            false,
            false,
            false,
            "toolchain-a",
        );
        assert_eq!(a, b);
    }

    #[test]
    // Single-char names are idiomatic for hash-comparison test fixtures.
    #[allow(clippy::many_single_char_names)]
    fn hash_ignores_unreferenced_upstream_types() {
        // A cell that consumes nothing hashes identically no matter what its
        // upstream cells produced — or where in the notebook it sits.
        let key = |types: &[String]| {
            content_hash_with_fingerprint(
                "40 + 2", "", types, None, None, false, false, false, "tc",
            )
        };
        let a = key(&[]);
        let b = key(&["u32".to_string(), "String".to_string()]);
        assert_eq!(
            a, b,
            "independent cells are cache-portable across notebooks"
        );

        // A referenced slot's type still matters…
        let key_ref = |types: &[String]| {
            content_hash_with_fingerprint(
                "cell1 * 2",
                "",
                types,
                None,
                None,
                false,
                false,
                false,
                "tc",
            )
        };
        let c = key_ref(&["u32".to_string(), "u32".to_string()]);
        let d = key_ref(&["u32".to_string(), "f64".to_string()]);
        assert_ne!(c, d, "the referenced slot's type is part of the key");
        // …while an unreferenced sibling's does not.
        let e = key_ref(&["String".to_string(), "u32".to_string()]);
        assert_eq!(c, e, "slot 0 is unreferenced and cannot fork the key");
    }

    #[test]
    fn hash_is_64_hex_chars() {
        let h = content_hash_with_fingerprint("x", "y", &[], None, None, false, false, false, "tc");
        assert_eq!(h.len(), 64);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
