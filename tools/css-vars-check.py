#!/usr/bin/env python3
"""Fail when the stylesheet reads a CSS custom property nobody defines.

`var(--ip-bg)` with no fallback resolves to NOTHING when `--ip-bg` is not
declared: the browser drops the whole declaration, silently. That shipped the
version-history panel with a transparent background over the notebook behind
it, which is unreadable, and neither clippy (it does not parse SCSS) nor
Playwright (it never asserts colours) could see it. The failure mode is
invisible until someone opens the panel, which is exactly the kind of thing a
guard should carry instead of a human.

A `var(--x, fallback)` is fine even when `--x` is undefined: the fallback is
the declared intent. Only bare reads are reported.

Fix: use a name from the palette at the top of `style/main.scss`, or declare
the new one there in BOTH themes.
"""

import pathlib
import re
import sys

SHEET = pathlib.Path("style/main.scss")

DEFINE_RE = re.compile(r"^\s*(--[a-zA-Z0-9_-]+)\s*:", re.M)
# Captures the name and whether a comma (i.e. a fallback) follows it.
USE_RE = re.compile(r"var\(\s*(--[a-zA-Z0-9_-]+)\s*(,)?")


def scan(text: str) -> list[tuple[int, str]]:
    defined = set(DEFINE_RE.findall(text))
    hits = []
    for num, line in enumerate(text.split("\n"), 1):
        for name, fallback in USE_RE.findall(line):
            if not fallback and name not in defined:
                hits.append((num, name))
    return hits


def main() -> int:
    if not SHEET.exists():
        print(f"{SHEET}: not found")
        return 1

    hits = scan(SHEET.read_text())
    for num, name in hits:
        print(f"{SHEET}:{num}: {name} is read but never defined")

    if hits:
        print(
            f"\n{len(hits)} undefined custom propert(y/ies). An undefined "
            "var() with no fallback drops the declaration entirely — the rule "
            "silently does nothing. Use a name from the palette at the top of "
            f"{SHEET}, or declare it there in both themes."
        )
        return 1

    print(f"{SHEET}: every var() resolves to a declared property")
    return 0


if __name__ == "__main__":
    sys.exit(main())
