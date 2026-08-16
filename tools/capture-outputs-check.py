#!/usr/bin/env python3
"""Fail when a public notebook's compiled text changed without a recapture.

`tools/capture-outputs.mjs` records, per notebook, a `cells` map (sha256 of
every runnable code cell's source + its per-cell Cargo.toml, 16 hex) and a
`shared` digest of the notebook-wide compile context (notebook
shared_source, shared cells' sources in order, shared Cargo.toml) into
`public/notebooks/.capture-manifest.json` as it captures. This check
recomputes the SAME recipe from the current notebooks and diffs: a mismatch
means someone edited text that compiles into cells and the committed
`saved_output` snapshots (PRD-0056) now show a run of code that no longer
exists. Keep the two recipes in lockstep — the committed manifest is the
cross-check between them (unilateral drift on either side fails ci loudly).

The manifest records capture-time sources for EVERY runnable cell, including
cells that produced no output (deliberate compile-fail teaching cells): the
capture ran them, and their text is part of what the snapshot set is honest
against. Notebooks with no runnable cells still get an entry (empty `cells`
map) so a markdown-only notebook cannot read as permanently stale.

Linux cells (PRD-0066) are outside the manifest by the same "Code only" rule
that excludes markdown and shared cells, and that exclusion is only half the
invariant: running one boots a metered BrowserPod pod, so `capture-outputs`
never clicks Run on one and a Linux cell must therefore never carry a
`saved_output`. A snapshot on one could only have arrived by hand or by a
regression in the capture tool, and nothing downstream would say so — hence
the explicit check below rather than a comment.

Fix: `cargo make capture-outputs -- <name>` against a dev server, then
commit the refreshed notebook + manifest.
"""

import hashlib
import json
import pathlib
import sys

DIR = pathlib.Path("public/notebooks")
MANIFEST = DIR / ".capture-manifest.json"


def digest(text: str) -> str:
    return hashlib.sha256(text.encode()).hexdigest()[:16]


def notebook_entry(path: pathlib.Path) -> dict:
    nb = json.loads(path.read_text())
    cells = {}
    for cell in nb.get("cells", []):
        if cell.get("cell_type", "Code") != "Code" or cell.get("shared"):
            continue
        cells[cell["id"]] = digest(
            f"{cell['source']}\x00{cell.get('cargo_toml') or ''}"
        )
    # NUL-joined so ("a", "b") and ("ab", "") cannot collide across parts.
    shared_parts = [nb.get("shared_source") or ""]
    shared_parts += [
        c["source"] for c in nb.get("cells", []) if c.get("shared")
    ]
    shared_parts.append(nb.get("shared_cargo_toml") or "")
    return {"cells": cells, "shared": digest("\x00".join(shared_parts))}


def linux_snapshots(path: pathlib.Path) -> list[str]:
    """Ids of Linux cells carrying a `saved_output`, which must be none."""
    nb = json.loads(path.read_text())
    return [
        cell["id"]
        for cell in nb.get("cells", [])
        if cell.get("cell_type") == "Linux" and cell.get("saved_output")
    ]


def main() -> int:
    if not MANIFEST.is_file():
        print(f"missing {MANIFEST}: run `cargo make capture-outputs` to create it")
        return 1
    manifest = json.loads(MANIFEST.read_text())

    stale: list[str] = []
    pods: list[str] = []
    for path in sorted(DIR.glob("*.ironpad")):
        name = path.stem
        current = notebook_entry(path)
        recorded = manifest.get(name)
        if recorded != current:
            stale.append(name)
        pods += [f"{name}:{cell_id}" for cell_id in linux_snapshots(path)]

    orphans = sorted(set(manifest) - {p.stem for p in DIR.glob("*.ironpad")})

    if stale or orphans or pods:
        for name in stale:
            print(f"stale saved outputs: {name} (compiled text changed since last capture)")
        for name in orphans:
            print(f"manifest entry for deleted notebook: {name}")
        for ref in pods:
            print(
                f"Linux cell carries a saved_output: {ref} — Linux cells are "
                "never captured (running one boots a metered pod); delete it"
            )
        if stale or orphans:
            print(
                "\nrun `cargo make capture-outputs -- <name> ...` against a dev "
                "server, then commit the notebook + manifest"
            )
        return 1

    print(f"capture manifest is fresh ({len(manifest)} notebooks)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
