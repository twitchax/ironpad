#!/usr/bin/env python3
"""Content lint for public .ironpad notebooks.

Checks the authoring invariants that CI's compile gate can't see:
  - valid JSON with sequential cell `order`
  - no em-dashes (voice rule) and no emoji in prose, titles, descriptions
  - no literal `.await` in Code cells that also use the `blocking::` API
    (the async wrapper it triggers cannot enter the JSPI promising path;
    see CLAUDE.md "Cell Special Cases" — plain async cells are fine)
  - `blog`-tagged notebooks report their markdown word count

Usage: python3 tools/lint-notebooks.py [file ...]   (default: all public notebooks)
Exits non-zero on any violation.
"""

import glob
import json
import sys
import unicodedata

EM_DASH = "—"


def is_emoji(ch: str) -> bool:
    return unicodedata.category(ch) == "So" and ord(ch) >= 0x1F000


def lint(path: str) -> list[str]:
    errors = []
    try:
        nb = json.load(open(path))
    except Exception as e:  # noqa: BLE001 - report any parse failure as a lint error
        return [f"invalid JSON: {e}"]

    for field in ("title", "description"):
        text = nb.get(field) or ""
        if EM_DASH in text:
            errors.append(f"em-dash in {field}")
        if any(is_emoji(ch) for ch in text):
            errors.append(f"emoji in {field}")

    orders = [c.get("order") for c in nb.get("cells", [])]
    if orders != list(range(len(orders))):
        errors.append(f"cell order not sequential: {orders}")

    for c in nb.get("cells", []):
        cid = c.get("id", "?")
        src = c.get("source", "")
        if EM_DASH in src:
            errors.append(f"em-dash in cell {cid}")
        if any(is_emoji(ch) for ch in src):
            errors.append(f"emoji in cell {cid}")
        blocking_cell = "blocking::" in src or "ironpad_blocking_" in src
        if c.get("cell_type") == "Code" and blocking_cell and ".await" in src:
            errors.append(
                f"literal .await in blocking Code cell {cid} "
                "(async wrapper cannot enter the JSPI promising path)"
            )

    return errors


def main() -> int:
    paths = sys.argv[1:] or sorted(glob.glob("public/notebooks/*.ironpad"))
    failed = False
    for path in paths:
        errors = lint(path)
        nb = None
        try:
            nb = json.load(open(path))
        except Exception:  # noqa: BLE001
            pass
        words = (
            sum(
                len(c["source"].split())
                for c in nb["cells"]
                if c.get("cell_type") == "Markdown"
            )
            if nb
            else 0
        )
        tag = " [blog]" if nb and "blog" in (nb.get("tags") or []) else ""
        status = "ok" if not errors else "FAIL"
        print(f"{status:4} {path} md_words={words}{tag}")
        for e in errors:
            print(f"     - {e}")
            failed = True
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
