#!/usr/bin/env python3
"""Generate public/monaco/completions-index.json from ironpad-cell's source.

Scans the crate cells actually link against for `pub` items (functions,
structs, enums, traits, constants) plus `pub fn` methods inside impl blocks,
carrying each item's doc-comment first paragraph and its real signature line.

A source scan instead of rustdoc JSON on purpose: ironpad-cell is ours and
consistently styled, the signature text comes out exactly as written, and the
generator has no coupling to rustdoc's unstable JSON format. Regenerate with
`cargo make gen-completions` whenever the ironpad-cell API surface changes;
the output is committed so builds don't depend on this script.
"""

import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "crates" / "ironpad-cell" / "src"
OUT = ROOT / "public" / "monaco" / "completions-index.json"

# Modules whose public items cells call through a path (module re-exported by
# the prelude), and modules whose items land in scope directly.
PATH_MODULES = {"blocking": "blocking", "sim": "sim", "ui": "ui"}
SKIP_FILES = {"enzyme_shims.rs"}

ITEM_RE = re.compile(
    r"^(?P<indent>\s*)pub\s+(?:async\s+)?"
    r"(?P<kind>fn|struct|enum|trait|const|type)\s+"
    r"(?P<name>[A-Za-z_][A-Za-z0-9_]*)"
)
IMPL_RE = re.compile(r"^impl(?:<[^>]*>)?\s+(?P<ty>[A-Za-z_][A-Za-z0-9_]*)(?:<[^>]*>)?\s*(?:\{|$)")
TRAIT_IMPL_RE = re.compile(r"^impl(?:<[^>]*>)?\s+\S+\s+for\s+")

KIND_MAP = {
    "fn": "function",
    "struct": "struct",
    "enum": "enum",
    "trait": "interface",
    "const": "constant",
    "type": "struct",
}


def first_doc_paragraph(lines, idx):
    """Doc-comment paragraph immediately above lines[idx], joined."""
    doc = []
    j = idx - 1
    while j >= 0:
        stripped = lines[j].strip()
        if stripped.startswith("///"):
            doc.append(stripped[3:].strip())
            j -= 1
        elif stripped.startswith("#["):
            j -= 1  # attributes sit between docs and item
        else:
            break
    doc.reverse()
    # First paragraph only.
    para = []
    for line in doc:
        if not line:
            break
        para.append(line)
    return " ".join(para)


def signature(lines, idx):
    """The item's signature: its line (and continuations) up to `{`/`;`."""
    sig = []
    for line in lines[idx : idx + 4]:
        text = line.strip()
        cut = len(text)
        for stop in ("{", ";"):
            pos = text.find(stop)
            if pos != -1:
                cut = min(cut, pos)
        sig.append(text[:cut].strip())
        if cut != len(text):
            break
    return " ".join(s for s in sig if s)


def scan_file(path):
    lines = path.read_text().splitlines()
    module = path.stem  # e.g. "blocking", "lib"
    items = []
    impl_ty = None
    impl_indent = None

    for i, line in enumerate(lines):
        # Track inherent-impl context (trait impls add no callable surface
        # beyond the trait, and operator impls are noise).
        m_impl = IMPL_RE.match(line)
        if m_impl and not TRAIT_IMPL_RE.match(line):
            impl_ty = m_impl.group("ty")
            impl_indent = len(line) - len(line.lstrip())
            continue
        if impl_ty is not None and line.strip() == "}" and (len(line) - len(line.lstrip())) == impl_indent:
            impl_ty = None
            continue

        m = ITEM_RE.match(line)
        if not m:
            continue
        name = m.group("name")
        kind = m.group("kind")
        indent = len(m.group("indent"))

        doc = first_doc_paragraph(lines, i)
        sig = signature(lines, i)

        if impl_ty is not None and kind == "fn" and indent > 0:
            # Method (or associated fn): completes as `name` with the owner
            # shown, plus a `Type::name` entry for associated construction.
            items.append({
                "label": f"{impl_ty}::{name}",
                "insert": name if "(&self" in sig or "&mut self" in sig or "(self" in sig else f"{impl_ty}::{name}",
                "kind": "method",
                "detail": sig,
                "doc": doc,
            })
        elif indent == 0:
            label = name
            insert = name
            if module in PATH_MODULES and kind == "fn":
                label = f"{PATH_MODULES[module]}::{name}"
                insert = label
            items.append({
                "label": label,
                "insert": insert,
                "kind": KIND_MAP[kind],
                "detail": sig,
                "doc": doc,
            })
    return items


def main():
    items = []
    for path in sorted(SRC.glob("*.rs")):
        if path.name in SKIP_FILES:
            continue
        items.extend(scan_file(path))

    # De-duplicate by (label, detail), keep first occurrence.
    seen = set()
    unique = []
    for item in items:
        key = (item["label"], item["detail"])
        if key in seen:
            continue
        seen.add(key)
        unique.append(item)

    OUT.write_text(json.dumps({"items": unique}, indent=1) + "\n")
    print(f"wrote {OUT.relative_to(ROOT)} with {len(unique)} items")


if __name__ == "__main__":
    main()
