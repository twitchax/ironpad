// Capture saved outputs into the public notebooks (PRD-0056 content pass).
//
// For each `public/notebooks/*.ironpad`: seed it into IndexedDB as a scratch
// /local notebook, Run All, wait for the run queue to drain, then use the
// app's own **Download .ironpad** flow — the production capture path, which
// embeds `saved_output` via `embed_saved_outputs` — and write the downloaded
// file back over the source, preserving its id and timestamps.
//
// Cells that fail to run simply keep no snapshot; the viewer renders them
// exactly as it does today.
//
// Usage:
//   node tools/capture-outputs.mjs                 # every notebook
//   node tools/capture-outputs.mjs cannon autodiff # a subset
//
// Requires a server on :3111 (`cargo make dev` or the release binary). It
// shares that server's compile cache, so a warm cache is fast and a cold one
// pays full build cost per distinct cell.

import { chromium } from "@playwright/test";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const BASE = process.env.IRONPAD_BASE ?? "http://localhost:3111";
const DIR = "public/notebooks";
/**
 * Freshness manifest: per notebook, `cells` maps every runnable code cell's
 * id to a capture-time sha256 (16 hex) of its source + its per-cell
 * Cargo.toml, and `shared` digests the notebook-wide compile context
 * (notebook shared_source, shared cells' sources in order, and the shared
 * Cargo.toml) — all of it compiles into every runnable cell, so editing any
 * of it stales every snapshot in the notebook. `tools/capture-outputs-check.py`
 * (in ci) recomputes the SAME recipe from the current notebooks and diffs,
 * so editing compiled text without recapturing fails the build instead of
 * silently shipping snapshots of code that no longer exists. Keep the two
 * recipes in lockstep — the committed manifest is the cross-check between
 * them (unilateral drift on either side fails ci loudly).
 */
const MANIFEST = path.join(DIR, ".capture-manifest.json");

const cellHash = (source) =>
  crypto.createHash("sha256").update(source).digest("hex").slice(0, 16);

const runnableHashes = (nb) =>
  Object.fromEntries(
    nb.cells
      .filter((c) => (c.cell_type ?? "Code") === "Code" && !c.shared)
      .map((c) => [c.id, cellHash(`${c.source}\u0000${c.cargo_toml ?? ""}`)]),
  );

// NUL-joined so ("a", "b") and ("ab", "") cannot collide across parts.
const sharedContextHash = (nb) =>
  cellHash(
    [
      nb.shared_source ?? "",
      ...nb.cells.filter((c) => c.shared).map((c) => c.source),
      nb.shared_cargo_toml ?? "",
    ].join("\u0000"),
  );

function recordManifest(name, nb) {
  const manifest = fs.existsSync(MANIFEST)
    ? JSON.parse(fs.readFileSync(MANIFEST, "utf-8"))
    : {};
  manifest[name] = { cells: runnableHashes(nb), shared: sharedContextHash(nb) };
  const sorted = Object.fromEntries(
    Object.keys(manifest)
      .sort()
      .map((k) => [k, manifest[k]]),
  );
  fs.writeFileSync(MANIFEST, `${JSON.stringify(sorted, null, 2)}\n`);
}
/** Budget per runnable cell for the run-all drain. */
const CELL_BUDGET_MS = Number(process.env.CELL_BUDGET_MS ?? 180_000);

// --allow-empty overrides the zero-capture guard below: without it, a run
// that captures NOTHING for a notebook whose committed file holds snapshots
// is treated as an environmental failure (broken server, missing toolchain,
// blocked registry) rather than a truth to record.
const ALLOW_EMPTY = process.argv.includes("--allow-empty");
const only = process.argv.slice(2).filter((a) => !a.startsWith("--"));
const files = fs
  .readdirSync(DIR)
  .filter((f) => f.endsWith(".ironpad"))
  .filter((f) => only.length === 0 || only.includes(f.replace(/\.ironpad$/, "")))
  .sort();

if (files.length === 0) {
  console.error("no matching notebooks");
  process.exit(1);
}

const browser = await chromium.launch();
const results = [];

for (const file of files) {
  const full = path.join(DIR, file);
  const source = JSON.parse(fs.readFileSync(full, "utf-8"));
  const runnable = source.cells.filter(
    (c) => (c.cell_type ?? "Code") === "Code" && !c.shared,
  ).length;
  if (runnable === 0) {
    // Still record a manifest entry: the checker computes one for every
    // notebook on disk, and a missing entry reads as permanently stale —
    // a markdown-only notebook would brick ci with no tool path to fix it.
    recordManifest(file.replace(/\.ironpad$/, ""), source);
    results.push({ file, status: "skipped (no runnable cells)" });
    console.log(JSON.stringify(results.at(-1)));
    continue;
  }

  const context = await browser.newContext({ acceptDownloads: true });
  const page = await context.newPage();
  const started = Date.now();
  try {
    await page.goto(`${BASE}/`);
    await page.waitForSelector(".ironpad-home", { timeout: 30_000 });
    await page.waitForTimeout(2_000); // hydration

    const scratchId = await page.evaluate(async (nb) => {
      const copy = { ...nb, id: crypto.randomUUID() };
      // Strip prior snapshots from the seed: Download PRESERVES saved_output
      // for cells with no fresh capture (PRD-0056), so a stale snapshot
      // seeded in would ride back out under this session's new source
      // hashes — an edited-to-failing cell would ship its old code's output
      // behind a green freshness check. An empty slate means the download
      // reflects only cells that really ran here.
      copy.cells = copy.cells.map((c) => {
        const cell = { ...c };
        delete cell.saved_output;
        return cell;
      });
      await window.IronpadStorage.saveNotebook(copy);
      return copy.id;
    }, source);

    await page.goto(`${BASE}/local/${scratchId}`);
    await page.waitForSelector(".ironpad-cell-card", { timeout: 30_000 });
    await page.waitForTimeout(2_000);

    // Run cells ONE AT A TIME rather than Run All: several teaching
    // notebooks open with a deliberate compile-fail cell, and the run-all
    // cascade correctly aborts the queue there — which would leave every
    // later cell in the notebook without a snapshot. A per-cell run still
    // cascades that cell's own prerequisites.
    //
    // Iterate ROWS, not run buttons. `.ironpad-cell-row` is one per cell in
    // notebook order, so row i is cell i whatever that cell renders inside
    // it; a flat `button[title="Run cell"]` list is only the SUBSET of cells
    // that offer one, and its indices say nothing about which cell they
    // belong to. That distinction is now load-bearing: clicking Run on a
    // Linux cell (PRD-0066) boots a metered BrowserPod pod, which is exactly
    // what capture must never do — Linux cells carry no `saved_output`, and
    // capturing one would need the API key to exist in CI. The alignment is
    // asserted rather than assumed, so a future change to the cell list
    // fails this tool loudly instead of clicking blind.
    const rows = page.locator(".ironpad-cell-row");
    const rowCount = await rows.count();
    if (rowCount !== source.cells.length) {
      throw new Error(
        `cell rows (${rowCount}) do not match cells (${source.cells.length}); ` +
          "the row-to-cell mapping this tool clicks through is broken",
      );
    }
    for (let i = 0; i < rowCount; i += 1) {
      const cell = source.cells[i];
      if ((cell.cell_type ?? "Code") !== "Code" || cell.shared) continue;
      const runButton = rows.nth(i).locator('button[title="Run cell"]');
      if ((await runButton.count()) === 0) continue;
      await runButton.click();
      const deadline = Date.now() + CELL_BUDGET_MS;
      for (;;) {
        const busy = await page.evaluate(
          () =>
            document.querySelectorAll(
              ".ironpad-cell-status--compiling, .ironpad-cell-status--running, .ironpad-cell-status--queued",
            ).length,
        );
        // A cell that fails to compile settles into Error; either way the
        // absence of busy cells means this run is done.
        if (busy === 0) break;
        if (Date.now() > deadline) throw new Error(`cell ${i} timed out`);
        await page.waitForTimeout(1_500);
      }
      await page.waitForTimeout(500);
    }
    await page.waitForTimeout(2_000); // let the last output settle

    // The production capture path.
    const downloadPromise = page.waitForEvent("download", { timeout: 60_000 });
    await page
      .locator('.ironpad-toolbar-dropdown-toggle[title="Notebook menu"]')
      .click();
    await page
      .locator(".ironpad-toolbar-dropdown-item", { hasText: "Download .ironpad" })
      .click();
    const download = await downloadPromise;
    const tmp = path.join(os.tmpdir(), `ironpad-capture-${Date.now()}.ironpad`);
    await download.saveAs(tmp);
    const enriched = JSON.parse(fs.readFileSync(tmp, "utf-8"));
    fs.unlinkSync(tmp);

    const outputs = new Map(
      enriched.cells.filter((c) => c.saved_output).map((c) => [c.id, c.saved_output]),
    );
    const captured = outputs.size;
    const hadSnapshots = source.cells.filter((c) => c.saved_output).length;

    if (captured === 0 && hadSnapshots > 0 && !ALLOW_EMPTY) {
      // Zero captures against a file that HAS snapshots is always
      // anomalous (every committed notebook captures at least one cell):
      // an environmental failure — server missing a pinned toolchain,
      // blocked registry — settles every cell into Error without throwing,
      // and recording it would strip the whole snapshot corpus under a
      // green freshness check. Keep the file, fail the run.
      process.exitCode = 1;
      results.push({
        file,
        status: `SUSPECT: 0 captured but the committed file holds ${hadSnapshots} snapshot(s) — kept as-is (rerun with --allow-empty to force)`,
        seconds: Math.round((Date.now() - started) / 1000),
      });
    } else {
      // Write saved_output INTO THE SOURCE object rather than writing the
      // round-tripped notebook: a round trip also materializes serde
      // defaults (version, collapse flags) the hand-written files omit,
      // which would bury the real change in noise. The diff is exactly the
      // saved_output fields. Because the seed was stripped, `outputs` holds
      // only this session's real runs — a cell that failed here loses its
      // stale snapshot (the delete arm).
      const before = JSON.stringify(source);
      for (const cell of source.cells) {
        const panels = outputs.get(cell.id);
        if (panels) {
          cell.saved_output = panels;
        } else {
          delete cell.saved_output;
        }
      }
      if (JSON.stringify(source) !== before) {
        fs.writeFileSync(full, `${JSON.stringify(source, null, 2)}\n`);
      }
      // This WAS a capture: the cells ran (or deliberately failed); record
      // the sources the attempt was made against so the freshness check
      // holds.
      recordManifest(file.replace(/\.ironpad$/, ""), source);
      results.push({
        file,
        status:
          captured === 0
            ? `no outputs captured (${runnable} runnable)`
            : `captured ${captured}/${runnable}`,
        seconds: Math.round((Date.now() - started) / 1000),
      });
    }
  } catch (e) {
    process.exitCode = 1;
    results.push({
      file,
      status: `FAILED: ${e.message}`,
      seconds: Math.round((Date.now() - started) / 1000),
    });
  } finally {
    await context.close();
  }
  console.log(JSON.stringify(results.at(-1)));
}

await browser.close();
console.log("\n=== summary ===");
for (const r of results) {
  console.log(`${r.file}: ${r.status}${r.seconds ? ` (${r.seconds}s)` : ""}`);
}
