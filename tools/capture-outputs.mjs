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
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const BASE = process.env.IRONPAD_BASE ?? "http://localhost:3111";
const DIR = "public/notebooks";
/** Budget per runnable cell for the run-all drain. */
const CELL_BUDGET_MS = Number(process.env.CELL_BUDGET_MS ?? 180_000);

const only = process.argv.slice(2);
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
      await window.IronpadStorage.saveNotebook(copy);
      return copy.id;
    }, source);

    await page.goto(`${BASE}/local/${scratchId}`);
    await page.waitForSelector(".ironpad-cell-card", { timeout: 30_000 });
    await page.waitForTimeout(2_000);

    await page.locator(".ironpad-run-all-button").click();
    const deadline = Date.now() + CELL_BUDGET_MS * runnable;
    for (;;) {
      const busy = await page.evaluate(
        () =>
          document.querySelectorAll(
            ".ironpad-cell-status--compiling, .ironpad-cell-status--running, .ironpad-cell-status--queued",
          ).length,
      );
      if (busy === 0) break;
      if (Date.now() > deadline) throw new Error("run-all timed out");
      await page.waitForTimeout(2_000);
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

    const captured = enriched.cells.filter((c) => c.saved_output).length;
    if (captured === 0) {
      results.push({
        file,
        status: `no outputs captured (${runnable} runnable)`,
        seconds: Math.round((Date.now() - started) / 1000),
      });
    } else {
      // Only outputs change: keep the notebook's identity and timestamps so
      // the diff is exactly the new saved_output fields.
      enriched.id = source.id;
      enriched.created_at = source.created_at;
      enriched.updated_at = source.updated_at;
      fs.writeFileSync(full, `${JSON.stringify(enriched, null, 2)}\n`);
      results.push({
        file,
        status: `captured ${captured}/${runnable}`,
        seconds: Math.round((Date.now() - started) / 1000),
      });
    }
  } catch (e) {
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
