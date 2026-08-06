#!/usr/bin/env python3
"""Fail when a public notebook's code changed without an output recapture.

`tools/capture-outputs.mjs` records a sha256 of every runnable code cell's
source into `public/notebooks/.capture-manifest.json` as it captures. This
check recomputes those hashes from the current notebooks and diffs: a
mismatch means someone edited a cell's code and the committed `saved_output`
snapshots (PRD-0056) now show a run of code that no longer exists.

The manifest records capture-time sources for EVERY runnable cell, including
cells that produced no output (deliberate compile-fail teaching cells): the
capture ran them, and their text is part of what the snapshot set is honest
against.

Fix: `cargo make capture-outputs -- <name>` against a dev server, then
commit the refreshed notebook + manifest.
"""

import hashlib
import json
import pathlib
import sys

DIR = pathlib.Path("public/notebooks")
MANIFEST = DIR / ".capture-manifest.json"


def runnable_hashes(path: pathlib.Path) -> dict[str, str]:
    nb = json.loads(path.read_text())
    out = {}
    for cell in nb.get("cells", []):
        if cell.get("cell_type", "Code") != "Code" or cell.get("shared"):
            continue
        digest = hashlib.sha256(cell["source"].encode()).hexdigest()[:16]
        out[cell["id"]] = digest
    return out


def main() -> int:
    if not MANIFEST.is_file():
        print(f"missing {MANIFEST}: run `cargo make capture-outputs` to create it")
        return 1
    manifest = json.loads(MANIFEST.read_text())

    stale: list[str] = []
    for path in sorted(DIR.glob("*.ironpad")):
        name = path.stem
        current = runnable_hashes(path)
        recorded = manifest.get(name)
        if recorded != current:
            stale.append(name)

    orphans = sorted(set(manifest) - {p.stem for p in DIR.glob("*.ironpad")})

    if stale or orphans:
        for name in stale:
            print(f"stale saved outputs: {name} (code changed since last capture)")
        for name in orphans:
            print(f"manifest entry for deleted notebook: {name}")
        print(
            "\nrun `cargo make capture-outputs -- <name> ...` against a dev "
            "server, then commit the notebook + manifest"
        )
        return 1

    print(f"capture manifest is fresh ({len(manifest)} notebooks)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
