#!/usr/bin/env python3
"""Fail when a bare Unicode symbol glyph is rendered in the UI (PRD-0062).

ironpad draws its affordances with a shipped icon set, not with characters,
because a character's shape comes from whatever font the user's machine has:
five glyphs used to render as full-colour emoji, nine more coloured
themselves on some Windows/Android configurations, and the rest came from
DejaVu Sans, Segoe UI Symbol, or Apple Symbols depending on platform.

The migration is only worth doing once, so this guard keeps the next feature
from quietly reintroducing one. It is the enforcement half of the same
pattern `gen-completions-check` and `capture-outputs-check` follow: state the
invariant in code rather than in a comment nobody reads.

Scope: non-comment lines of the crates' `src/` trees and `public/*.js`, up to
the first `#[cfg(test)]` (test fixtures legitimately contain emoji and
multibyte text — that is what several of them are testing).

Escape hatch: append `glyph-check: allow` in a comment on the offending line
when a symbol really is content rather than an affordance.

Fix: add a role to `crates/ironpad-app/src/components/icons.rs` and render it
with `<Icon>` / `<IconLabel>` (or `icon_svg_markup` in a string context).
"""

import pathlib
import re
import sys
import unicodedata

ALLOW_MARKER = "glyph-check: allow"
COMMENT_PREFIXES = ("///", "//!", "//", "*", "/*", "#")

# NOTE: box-drawing characters (U+2500\u2013U+257F) are NOT exempt. They rule off
# comment sections everywhere in this codebase, but comment lines are already
# skipped below, so the exemption bought nothing and cost three real hits:
# `\u2573 Delete` (U+2573) shipped on two menu items and a card while the rest of
# the UI had moved to icons. An exemption that only ever fires on false
# negatives is not an exemption, it is a hole.

# Rust `\u{1f5c2}` and JS `\u25cf`. Checking only literal characters missed a
# colour emoji and seven other affordances hiding in escape form during the
# PRD-0062 migration — an invariant a rewrite can sidestep is not enforced.
ESCAPE_RE = re.compile(r"\\u\{([0-9a-fA-F]{2,6})\}|\\u([0-9a-fA-F]{4})")

# Vendored bundles are not ours to rewrite (KaTeX ships thousands of maths
# symbols by design).
VENDORED = ("monaco", "katex", "prism", "sortable", ".min.js")


def strip_trailing_comment(line: str) -> str:
    """Drop a trailing `// …` so prose in it is not treated as rendered code.

    Skips `://` so a URL in a string survives. A `//` inside a string
    literal would truncate early (a false negative), which is the right way
    to be wrong for a guard whose cost of a false positive is a blocked
    build over a comment.
    """
    idx = 0
    while (idx := line.find("//", idx)) != -1:
        if idx > 0 and line[idx - 1] == ":":
            idx += 2
            continue
        return line[:idx]
    return line


def is_ui_symbol(cp: int) -> bool:
    return cp >= 0x2000 and unicodedata.category(chr(cp)).startswith("S")


def offending(line: str) -> list[str]:
    code = strip_trailing_comment(line)
    found = [chr(cp) for cp in map(ord, code) if is_ui_symbol(cp)]
    # ...and the same characters written as escapes, which read as ASCII.
    found += [
        chr(cp)
        for m in ESCAPE_RE.finditer(code)
        if is_ui_symbol(cp := int(m.group(1) or m.group(2), 16))
    ]
    return found


def scan(path: pathlib.Path) -> list[tuple[int, str, str]]:
    text = path.read_text()
    # Test modules may hold emoji/multibyte fixtures on purpose.
    cut = text.find("#[cfg(test)]")
    if cut != -1:
        text = text[:cut]
    hits = []
    for num, line in enumerate(text.split("\n"), 1):
        stripped = line.strip()
        if stripped.startswith(COMMENT_PREFIXES) or ALLOW_MARKER in line:
            continue
        found = offending(line)
        if found:
            hits.append((num, "".join(dict.fromkeys(found)), stripped[:90]))
    return hits


def main() -> int:
    targets: list[pathlib.Path] = []
    for crate in sorted(pathlib.Path("crates").glob("*/src")):
        targets.extend(sorted(crate.rglob("*.rs")))
    targets.extend(
        p
        for p in sorted(pathlib.Path("public").glob("*.js"))
        if not any(v in str(p) for v in VENDORED)
    )

    total = 0
    for path in targets:
        for num, glyphs, snippet in scan(path):
            total += 1
            print(f"{path}:{num}: bare glyph {glyphs} -> use an icons:: role")
            print(f"    {snippet}")

    if total:
        print(
            f"\n{total} bare glyph(s) in rendered source. Add a role to "
            "crates/ironpad-app/src/components/icons.rs and render it with "
            "<Icon>/<IconLabel>, or mark the line `glyph-check: allow` if it is "
            "genuinely content."
        )
        return 1

    print(f"no bare glyphs in rendered source ({len(targets)} files scanned)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
