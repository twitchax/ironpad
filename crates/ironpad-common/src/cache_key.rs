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
pub const CACHE_EPOCH: u32 = 8;

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
    let mut hasher = blake3::Hasher::new();
    update_framed(&mut hasher, source.as_bytes());
    update_framed(&mut hasher, cargo_toml.as_bytes());
    update_framed(&mut hasher, TARGET_TRIPLE.as_bytes());
    update_framed(&mut hasher, &(previous_types.len() as u64).to_le_bytes());
    for t in previous_types {
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

/// Extract dependency lines from the user's `Cargo.toml` content.
///
/// Finds the `[dependencies]` section and collects all lines until the next
/// section header (`[...]`), filtering out any existing `ironpad-cell` entry
/// (we always inject our own).
#[must_use]
pub fn extract_user_dependencies(cargo_toml: &str) -> String {
    let mut in_deps = false;
    let mut deps = Vec::new();

    for line in cargo_toml.lines() {
        let trimmed = line.trim();

        // Detect section headers.
        if trimmed.starts_with('[') {
            in_deps = trimmed == "[dependencies]";
            continue;
        }

        if in_deps && !trimmed.is_empty() && !trimmed.starts_with('#') {
            // Skip any user-specified ironpad-cell (we inject our own).
            if trimmed.starts_with("ironpad-cell") || trimmed.starts_with("ironpad_cell") {
                continue;
            }
            deps.push(line);
        }
    }

    deps.join("\n")
}

/// Merge shared (notebook-level) and cell-level dependencies.
///
/// Cell deps take precedence: if both shared and cell declare the same crate
/// name, the cell's line wins. The merge is at the dependency-line level.
#[must_use]
pub fn merge_dependencies(shared_cargo_toml: Option<&str>, cell_cargo_toml: &str) -> String {
    let shared_deps = shared_cargo_toml.map_or_else(String::new, extract_user_dependencies);
    let cell_deps = extract_user_dependencies(cell_cargo_toml);

    if shared_deps.is_empty() {
        return cell_deps;
    }
    if cell_deps.is_empty() {
        return shared_deps;
    }

    // Build a map of crate_name → dep_line, shared first, then cell overrides.
    let mut dep_map: Vec<(String, String)> = Vec::new();

    for line in shared_deps.lines() {
        if let Some(name) = crate_name_from_dep_line(line) {
            dep_map.push((name, line.to_string()));
        }
    }

    for line in cell_deps.lines() {
        if let Some(name) = crate_name_from_dep_line(line) {
            if let Some(entry) = dep_map.iter_mut().find(|(n, _)| *n == name) {
                entry.1 = line.to_string();
            } else {
                dep_map.push((name, line.to_string()));
            }
        }
    }

    dep_map
        .into_iter()
        .map(|(_, line)| line)
        .collect::<Vec<_>>()
        .join("\n")
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
    let merged = merge_dependencies(shared_cargo_toml, cell_cargo_toml);
    merged
        .lines()
        .filter_map(crate_name_from_dep_line)
        .any(|name| name == "rayon")
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

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

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
    fn hash_is_64_hex_chars() {
        let h = content_hash_with_fingerprint("x", "y", &[], None, None, false, false, false, "tc");
        assert_eq!(h.len(), 64);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
