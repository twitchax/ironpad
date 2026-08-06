//! Piped-input dependency detection, shared by the scaffold, the cache key,
//! the run cascade, and the CLI daemon (PRD-0060).
//!
//! A cell consumes upstream outputs ONLY through the injected `cellN`
//! bindings and the `last` alias, so its dependency set is knowable from its
//! source text. Detection is conservative in the safe direction: a false
//! positive (the string `cell2` in a comment) costs an extra binding and an
//! over-eager cascade — exactly today's behavior — never a wrong result.
//!
//! The recipe here MUST stay the single source of truth: the scaffold binds
//! the slots [`referenced_slots`] reports, the cache key hashes the types
//! [`normalize_previous_types`] keeps, and the cascade runs the cells
//! [`transitive_dep_indices`] returns. Forking any one of them desyncs
//! codegen from cache identity.

use std::collections::BTreeSet;

/// Slots a cell's source references: explicit `cellN` indices plus whether
/// the `last` alias appears. `last` binds to the last TYPED upstream slot at
/// compile time — a dynamic target — so a cell referencing it conservatively
/// depends on every upstream runnable cell.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SlotRefs {
    pub slots: BTreeSet<usize>,
    pub last: bool,
    /// The source touches the raw input buffer (`__ironpad_inputs__`)
    /// directly, bypassing the typed bindings — reads at arbitrary indices,
    /// so it conservatively depends on everything upstream.
    pub raw: bool,
}

impl SlotRefs {
    /// True when the source references no piped inputs at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty() && !self.last && !self.raw
    }

    /// The conservative arms: `last` aliases the last TYPED slot (a dynamic
    /// target) and raw-buffer access reads anywhere, so both depend on all
    /// upstream runnable cells.
    #[must_use]
    pub fn depends_on_all(&self) -> bool {
        self.last || self.raw
    }
}

/// Is `b` a character that extends an identifier (so a match next to it is a
/// fragment of a longer name, not a reference)?
fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Slot indices above this are ignored: notebooks are bounded far below it,
/// and it keeps a pathological `cell99999999999999` from parsing games.
const MAX_SLOT: usize = 10_000;

/// Scan `source` for piped-input references.
///
/// `cellN` matches with word boundaries on both sides (`mycell1` and
/// `cell1x` do not count; `cell10` is slot 10, not slot 1). `last` matches
/// with word boundaries and NOT preceded by `.` — `.last()` calls are the
/// overwhelmingly common false positive worth excluding; a user-defined
/// `last` variable still counts, which only over-cascades.
#[must_use]
pub fn referenced_slots(source: &str) -> SlotRefs {
    let bytes = source.as_bytes();
    let mut refs = SlotRefs::default();

    for (start, _) in source.match_indices("cell") {
        if start > 0 && is_ident_char(bytes[start - 1]) {
            continue;
        }
        let digits_start = start + 4;
        let mut end = digits_start;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
        if end == digits_start {
            continue; // "cell" with no digits — not a slot reference
        }
        if end < bytes.len() && is_ident_char(bytes[end]) {
            continue; // cell1x — identifier fragment
        }
        if let Ok(n) = source[digits_start..end].parse::<usize>() {
            if n <= MAX_SLOT {
                refs.slots.insert(n);
            }
        }
    }

    refs.raw = source.contains("__ironpad_inputs__");

    for (start, _) in source.match_indices("last") {
        if start > 0 {
            let prev = bytes[start - 1];
            if is_ident_char(prev) {
                continue; // `mylast`, `xlast` — identifier fragment
            }
            // A LONE preceding `.` is a method call (`x.last()`) and does not
            // reference the alias. But `..` is the range / struct-update
            // operator, whose second byte is also `.`, and `..last.clone()`
            // or `0..last.len()` DO reference the alias — excluding them
            // dropped a real dependency and omitted the `last` binding
            // (E0425). Only skip a `.` that is not the tail of a `..`.
            if prev == b'.' && !(start >= 2 && bytes[start - 2] == b'.') {
                continue;
            }
        }
        let end = start + 4;
        if end < bytes.len() && is_ident_char(bytes[end]) {
            continue;
        }
        refs.last = true;
        break;
    }

    refs
}

/// Project `previous_types` down to what the cell can actually consume, so
/// unreferenced upstream type tags stop forking the cache key (and the
/// scaffold's binding set stays consistent with cache identity).
///
/// A source referencing `last` keeps the full vector (the alias target
/// depends on the whole emptiness pattern). Otherwise unreferenced slots
/// blank out and trailing blanks truncate — an independent cell hashes
/// identically at index 7 of one notebook and index 2 of another, which is
/// what lets share snapshots and warm caches serve it anywhere.
#[must_use]
pub fn normalize_previous_types(source: &str, previous_types: &[String]) -> Vec<String> {
    let refs = referenced_slots(source);
    if refs.depends_on_all() {
        return previous_types.to_vec();
    }
    let mut out: Vec<String> = previous_types
        .iter()
        .enumerate()
        .map(|(i, tag)| {
            if refs.slots.contains(&i) {
                tag.clone()
            } else {
                String::new()
            }
        })
        .collect();
    while out.last().is_some_and(String::is_empty) {
        out.pop();
    }
    out
}

/// One cell as the dependency graph sees it: can it run, and what does its
/// source say (in notebook order, full list — slot N is index N).
///
/// `source: None` means "text unavailable" and degrades to depending on all
/// upstream runnable cells, today's behavior.
pub struct DepCell<'a> {
    pub runnable: bool,
    pub source: Option<&'a str>,
}

/// The transitive dependency closure of `target_idx`: every upstream
/// runnable cell the target consumes, directly or through another dependency.
/// Non-runnable slots (markdown, shared) never execute and are not expanded.
#[must_use]
pub fn transitive_dep_indices(cells: &[DepCell<'_>], target_idx: usize) -> BTreeSet<usize> {
    let mut deps = BTreeSet::new();
    if target_idx >= cells.len() {
        return deps;
    }
    let mut work = vec![target_idx];
    while let Some(i) = work.pop() {
        let direct: Vec<usize> = match cells[i].source {
            Some(src) => {
                let refs = referenced_slots(src);
                if refs.depends_on_all() {
                    (0..i).filter(|&j| cells[j].runnable).collect()
                } else {
                    refs.slots
                        .iter()
                        .copied()
                        .filter(|&j| j < i && cells[j].runnable)
                        .collect()
                }
            }
            None => (0..i).filter(|&j| cells[j].runnable).collect(),
        };
        for d in direct {
            if deps.insert(d) {
                work.push(d);
            }
        }
    }
    deps
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn slots(source: &str) -> Vec<usize> {
        referenced_slots(source).slots.into_iter().collect()
    }

    #[test]
    fn cell_references_match_word_boundaries() {
        assert_eq!(slots("let x = cell1 + cell2;"), vec![1, 2]);
        // cell10 is slot 10, not a match for slot 1.
        assert_eq!(slots("cell10"), vec![10]);
        // Identifier fragments never count.
        assert_eq!(slots("mycell1 cell2x sub_cell3"), Vec::<usize>::new());
        // Duplicates collapse; strings/comments still count (conservative).
        assert_eq!(slots("cell4 // cell4 again, and \"cell5\""), vec![4, 5]);
        // A bare "cell" with no digits is not a reference.
        assert_eq!(slots("let cell = 1; cell_count"), Vec::<usize>::new());
    }

    #[test]
    fn last_matches_the_alias_not_method_calls() {
        assert!(referenced_slots("let n = last.len();").last);
        assert!(referenced_slots("(last)").last);
        // .last() method calls are the common false positive — excluded.
        assert!(!referenced_slots("v.iter().last()").last);
        assert!(!referenced_slots("xs.last()").last);
        // ...but `..last` is the range / struct-update operator and DOES
        // reference the alias (its `.` is the tail of `..`, not a method dot).
        assert!(referenced_slots("Config { retries: 5, ..last.clone() }").last);
        assert!(referenced_slots("for i in 0..last.len() {}").last);
        assert!(referenced_slots("&v[0..last]").last);
        // Identifier fragments.
        assert!(!referenced_slots("let lastly = 1; blast!").last);
        // A shadowing user binding still counts: over-cascade, never break.
        assert!(referenced_slots("let last = compute();").last);
    }

    #[test]
    fn raw_input_buffer_access_is_conservative() {
        // `__ironpad_inputs__.get(N)` bypasses the typed bindings, so it
        // must behave like `last`: depend on everything, normalize nothing.
        let refs =
            referenced_slots("let d: Vec<i32> = __ironpad_inputs__.get(0).deserialize().unwrap();");
        assert!(refs.raw);
        assert!(refs.depends_on_all());

        let types = vec!["u32".to_string(), String::new()];
        assert_eq!(
            normalize_previous_types("__ironpad_inputs__.len()", &types),
            types
        );

        let cells = vec![
            DepCell {
                runnable: true,
                source: Some("1"),
            },
            DepCell {
                runnable: true,
                source: Some("__ironpad_inputs__.get(0)"),
            },
        ];
        let deps: Vec<usize> = transitive_dep_indices(&cells, 1).into_iter().collect();
        assert_eq!(deps, vec![0]);
    }

    #[test]
    fn normalization_blanks_unreferenced_and_truncates() {
        let types = vec![
            "u32".to_string(),
            "String".to_string(),
            "f64".to_string(),
            "Vec<u8>".to_string(),
        ];
        // Only slot 1 referenced: slot 0 blanks, trailing slots truncate.
        assert_eq!(
            normalize_previous_types("cell1 * 2", &types),
            vec![String::new(), "String".to_string()]
        );
        // Nothing referenced: fully empty — position-independent identity.
        assert_eq!(
            normalize_previous_types("40 + 2", &types),
            Vec::<String>::new()
        );
        // `last` keeps everything (dynamic alias target).
        assert_eq!(normalize_previous_types("*last", &types), types);
        // Referencing a slot past the end changes nothing.
        assert_eq!(
            normalize_previous_types("cell9", &types),
            Vec::<String>::new()
        );
    }

    #[test]
    fn transitive_closure_follows_references_through_runnable_cells() {
        // 0: code (no refs)   1: markdown   2: code refs cell0
        // 3: code (no refs)   4: code refs cell2
        let cells = vec![
            DepCell {
                runnable: true,
                source: Some("1 + 1"),
            },
            DepCell {
                runnable: false,
                source: Some("# prose about cell0"),
            },
            DepCell {
                runnable: true,
                source: Some("cell0 * 2"),
            },
            DepCell {
                runnable: true,
                source: Some("42"),
            },
            DepCell {
                runnable: true,
                source: Some("cell2 + 1"),
            },
        ];
        let deps: Vec<usize> = transitive_dep_indices(&cells, 4).into_iter().collect();
        assert_eq!(
            deps,
            vec![0, 2],
            "closure includes cell2's own dep, skips 1 and 3"
        );

        // An independent cell depends on nothing.
        assert!(transitive_dep_indices(&cells, 3).is_empty());

        // `last` is conservative: all upstream runnable cells.
        let with_last = vec![
            DepCell {
                runnable: true,
                source: Some("1"),
            },
            DepCell {
                runnable: true,
                source: Some("2"),
            },
            DepCell {
                runnable: true,
                source: Some("format!(\"{last:?}\")"),
            },
        ];
        let deps: Vec<usize> = transitive_dep_indices(&with_last, 2).into_iter().collect();
        assert_eq!(deps, vec![0, 1]);

        // Missing source degrades to all-upstream (today's behavior).
        let unknown = vec![
            DepCell {
                runnable: true,
                source: Some("1"),
            },
            DepCell {
                runnable: true,
                source: None,
            },
            DepCell {
                runnable: true,
                source: Some("cell1"),
            },
        ];
        let deps: Vec<usize> = transitive_dep_indices(&unknown, 2).into_iter().collect();
        assert_eq!(
            deps,
            vec![0, 1],
            "cell1's unknown source pulls in everything above it"
        );
    }

    #[test]
    fn out_of_range_target_is_empty() {
        assert!(transitive_dep_indices(&[], 0).is_empty());
        let one = vec![DepCell {
            runnable: true,
            source: Some("x"),
        }];
        assert!(transitive_dep_indices(&one, 5).is_empty());
    }
}
