//! Every `CompileRequest` must derive `cell_type` from the cell it is
//! compiling, never state it as a constant (PRD-0066).
//!
//! # Why this is a source scan and not a runtime test
//!
//! `cell_type` selects the whole compilation world: `Code` scaffolds a
//! fragment wrapped in `cell_main` for `wasm32-unknown-unknown` and our own
//! executor, `Linux` scaffolds a whole program for
//! `wasm32-browserpod-linux-musl` and an in-browser kernel. A request that
//! names the wrong one is not a bug the compile path can catch, because the
//! compile path does exactly what it was asked: the request lied.
//!
//! And the wrong answer does not fail loudly. Send a Linux cell with
//! `cell_type: Code` and the scaffold wraps the author's `fn main() { … }`
//! inside `cell_main`, where it becomes a nested `fn` — legal Rust, compiled
//! clean, never called. The build succeeds, the export-table assertions in
//! `compiler::e2e_tests` pass, the cell runs GREEN AND PRINTS NOTHING, and the
//! author has no error to act on. There is no layer below the request that can
//! tell it was wrong.
//!
//! So the thing worth asserting is the outcome — no cell ever reaches
//! `compile_cell` under another cell's type — rather than any one route to it.
//! The editor's Run button was one such route (it gates on
//! `is_markdown || is_shared`, and a Linux cell is neither); a test that
//! pinned the button's absence would have kept passing the moment a second
//! route appeared. This follows `server_fns::tests::admin_fns_are_all_gated`,
//! for the same reason it was written that way: a runtime test covers the call
//! sites someone remembered to list, and the failure that matters is the
//! `CompileRequest` literal written next year.
//!
//! # The stronger version, deliberately not built yet
//!
//! Making this unrepresentable — a constructor taking the cell and reading its
//! type — would delete the class of bug rather than test for it. It is not
//! done because the shapes do not all fit one constructor: the live-check path
//! substitutes `SHARED_CHECK_BODY` for the cell's source, and tests build
//! requests with no cell behind them at all, so the escape hatches would erode
//! the guarantee they exist to provide. If this test ever trips a second time,
//! build the constructor instead of widening the list below.

use std::path::{Path, PathBuf};

/// Call sites permitted to name a constant: file, HOW MANY, and the
/// construction that makes it true. Adding to this list must be a deliberate
/// edit to an assertion, which is the point of it being here rather than
/// inferred.
///
/// The count is load-bearing. A bare file-level exemption would let a SECOND
/// hardcoded literal appear in an already-exempt file and be waved through by
/// a reason written about a different line — the exact "an exemption that only
/// ever fires on false negatives is not an exemption, it is a hole" failure
/// this repo removed from `glyph-check`.
///
/// Test code is exempt wholesale (see [`strip_test_module`]): a synthetic
/// request has no cell behind it to derive from.
const HARDCODE_ALLOWED: &[(&str, usize, &str)] = &[
    (
        "crates/ironpad-app/src/components/view_only_notebook.rs",
        1,
        "its literal is inside `ViewOnlyCodeCell`, which is only reachable from \
         the `CellType::Code` arm of `ViewOnlyCell`'s dispatch",
    ),
    (
        "crates/ironpad-app/src/pages/notebook_editor/pipeline.rs",
        1,
        "the shared-cell live check is not a check of THIS cell: it substitutes \
         SHARED_CHECK_BODY and validates the notebook's shared source, which is \
         a Rust fragment and must go through the Code scaffold whatever the \
         host cell's own type is. Non-shared cells on that same line derive \
         normally. Deliberately NOT laundered through a `let` binding to quiet \
         this scan: hiding a constant behind a name would defeat the check",
    ),
];

/// How far past a literal's opening brace to look for its `cell_type` field.
/// Generous: the literals are around a dozen fields. Not finding one inside
/// this window is reported as scanner drift rather than passed over, because
/// `CompileRequest` has no `Default` and the compiler therefore guarantees
/// every literal writes the field.
const FIELD_SEARCH_LINES: usize = 60;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("the workspace root must resolve from this crate")
}

/// Every `.rs` file under a crate's `src/`, which is where shipped call sites
/// live. `tests/` trees are excluded for the same reason `#[cfg(test)]` is.
fn shipped_sources(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    let mut out = Vec::new();
    let crates = std::fs::read_dir(root.join("crates")).expect("crates/ must exist");
    for entry in crates.flatten() {
        let src = entry.path().join("src");
        if src.is_dir() {
            walk(&src, &mut out);
        }
    }
    out.sort();
    out
}

/// Drop everything from the first test module onward.
fn strip_test_module(src: &str) -> &str {
    let cut = ["#[cfg(test)]", "#[cfg(all(test"]
        .iter()
        .filter_map(|marker| src.find(marker))
        .min();
    cut.map_or(src, |at| &src[..at])
}

/// 1-based line number of a byte offset.
fn line_of(src: &str, at: usize) -> usize {
    src[..at].matches('\n').count() + 1
}

/// The `cell_type` field of the literal opening at `from`, as written.
///
/// Handles field-init shorthand (`cell_type,`) as well as `cell_type: expr`.
/// Shorthand is a derivation by definition — it names a binding, and a binding
/// cannot be a `CellType::` path — so it reads as the local's name and passes.
/// Missing this cost one red gate: `rustfmt`/clippy actively rewrite
/// `cell_type: cell_type` INTO the shorthand, so any scan that only understands
/// the long form will drift the moment someone runs the formatter.
fn cell_type_value(src: &str, from: usize) -> Option<(usize, String)> {
    let start_line = line_of(src, from);
    for (line_no, line) in (start_line..).zip(src[from..].lines().take(FIELD_SEARCH_LINES)) {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("cell_type:") {
            // Judged without its trailing comment: prose mentioning
            // `cell_type` beside a constant would otherwise vouch for it.
            let code = rest.split("//").next().unwrap_or(rest);
            return Some((line_no, code.trim().trim_end_matches(',').to_string()));
        }
        if t == "cell_type," {
            return Some((line_no, "cell_type".to_string()));
        }
    }
    None
}

#[test]
fn compile_requests_derive_cell_type_from_the_cell() {
    let root = workspace_root();
    let mut literals = 0usize;
    let mut exempt_counts: Vec<(&str, usize)> = HARDCODE_ALLOWED
        .iter()
        .map(|(file, _, _)| (*file, 0usize))
        .collect();
    let mut violations: Vec<String> = Vec::new();

    for path in shipped_sources(&root) {
        let raw = std::fs::read_to_string(&path).expect("a listed source must be readable");
        let src = strip_test_module(&raw);
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        let exempt_allowance = HARDCODE_ALLOWED
            .iter()
            .find(|(file, _, _)| *file == rel)
            .map(|(_, allowed, _)| *allowed);

        for (at, _) in src.match_indices("CompileRequest {") {
            // `struct CompileRequest {` and `impl CompileRequest {` wear the
            // same prefix as a literal. The type's own definition tripped this
            // scan's drift guard on the first run, which is the guard doing
            // its job — but the answer is to not match a definition, not to
            // loosen the guard.
            let before = &src[at.saturating_sub(16)..at];
            if before.ends_with("struct ") || before.ends_with("impl ") {
                continue;
            }
            literals += 1;
            let (line, value) = cell_type_value(src, at).unwrap_or_else(|| {
                panic!(
                    "{rel}:{}: a `CompileRequest` literal with no `cell_type` field within \
                     {FIELD_SEARCH_LINES} lines. `CompileRequest` has no `Default`, so the \
                     compiler guarantees the field exists — this scan has drifted from the code.",
                    line_of(src, at),
                )
            });

            // A constant names a world; anything else reads it off a cell.
            //
            // Deliberately blunt. A value can name a constant AND derive —
            // the live check reads `if shared { CellType::Code } else
            // { cell_type }`, correctly, because a shared cell's check
            // compiles SHARED_CHECK_BODY through the Code scaffold whatever
            // the host cell is. A rule that waved such values through
            // automatically was tried and rejected: it would equally have
            // accepted a constant sitting behind any expression that happened
            // to mention `cell_type`, and it moved the reasoning out of the
            // exemption list, where a reader looks, into this predicate,
            // where nobody does. Naming a constant costs an entry below.
            if !value.contains("CellType::") {
                continue;
            }
            if let Some(allowed) = exempt_allowance {
                let seen = exempt_counts
                    .iter_mut()
                    .find(|(file, _)| *file == rel)
                    .expect("an allowance implies a counter");
                seen.1 += 1;
                if seen.1 <= allowed {
                    continue;
                }
                // Past the allowance: report it like any other violation, so
                // the exemption cannot grow silently past what was reasoned
                // about.
            }
            violations.push(format!("  {rel}:{line}\n      writes `cell_type: {value}`"));
        }
    }

    assert!(
        violations.is_empty(),
        "{} `CompileRequest` literal(s) name a cell type instead of deriving it:\n\n{}\n\n\
         Derive it from the cell being compiled (`cell.cell_type.clone()`).\n\n\
         A constant `Code` on any path a Linux cell can reach does not fail: the scaffold \
         wraps the author's `fn main` into `cell_main`, where it is a nested fn that is \
         never called, so the build succeeds and the cell runs green with no output. \
         Nothing downstream can catch it, because the compile path did what the request \
         asked.\n\n\
         If a site is genuinely unreachable by any other cell type, add it to \
         HARDCODE_ALLOWED in crates/ironpad-common/tests/compile_request_cell_type.rs \
         together with the construction that makes it true.",
        violations.len(),
        violations.join("\n"),
    );

    // Guard the guard. A scan that matches nothing passes in silence, and
    // catching a call site added later is this test's entire purpose.
    assert!(
        literals >= 2,
        "found {literals} `CompileRequest` literal(s) in shipped source; the scan pattern \
         has drifted from the code"
    );

    // A stale exemption is a comment claiming to protect something it no
    // longer covers, so it fails rather than lingering — and an exemption for
    // more literals than exist is the same rot one step earlier.
    for (file, allowed, _) in HARDCODE_ALLOWED {
        let seen = exempt_counts
            .iter()
            .find(|(f, _)| f == file)
            .map_or(0, |(_, n)| *n);
        assert_eq!(
            seen, *allowed,
            "{file} is exempted for {allowed} hardcoded `CompileRequest` literal(s) but has \
             {seen}. Update the count, or remove the entry if the site now derives — an \
             exemption wider than the code it covers protects nothing and hides the next one."
        );
    }
}
