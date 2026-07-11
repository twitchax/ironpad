import { test, expect } from "@playwright/test";
import { setCellSource } from "./helpers/monaco";
import { trackJsErrors } from "./helpers/errors";
import { createNotebook } from "./helpers/session";

/**
 * E2E coverage for PRD-0042 (WASM SIMD cells) and PRD-0043 (JSPI blocking
 * host calls). Both compile fresh micro-crates server-side, so timeouts
 * mirror execution.spec.ts.
 */

/** Create a fresh notebook with one cell and return its locator. */
async function newNotebookWithCell(page) {
  await createNotebook(page);

  await page.locator(".ironpad-add-cell-btn").first().click();
  const cell = page.locator(".ironpad-cell-card").first();
  await expect(cell).toBeVisible();
  await expect(cell.locator(".monaco-editor").first()).toBeVisible({
    timeout: 15_000,
  });
  return cell;
}

/** Run the first cell and wait for a successful compile + execution. */
async function runCellToSuccess(page, cell) {
  const runButton = page.locator('button[title="Run cell"]').first();
  await expect(runButton).toBeVisible();
  await runButton.click();

  await expect(cell.locator(".ironpad-cell-status--compiling")).toBeVisible({
    timeout: 5_000,
  });
  await expect(cell.locator(".ironpad-cell-status--compiling")).toBeHidden({
    timeout: 120_000,
  });
  await expect(cell.locator(".ironpad-cell-status--success")).toBeVisible({
    timeout: 15_000,
  });
}

test.describe("SIMD cells (PRD-0042)", () => {
  test("portable SIMD cell compiles and computes in the browser", async ({
    page,
  }) => {
    test.setTimeout(180_000);
    const jsErrors = trackJsErrors(page);

    const cell = await newNotebookWithCell(page);
    await setCellSource(
      page,
      cell,
      [
        "use std::simd::prelude::*;",
        "let a = f32x4::from_array([1.0, 2.0, 3.0, 4.0]);",
        "let b = f32x4::from_array([5.0, 6.0, 7.0, 8.0]);",
        'CellOutput::text(format!("dot={}", (a * b).reduce_sum()))',
      ].join("\n")
    );

    await runCellToSuccess(page, cell);

    const outputText = cell.locator(".ironpad-output-display-text");
    await expect(outputText).toBeVisible({ timeout: 5_000 });
    await expect(outputText).toContainText("dot=70");

    expect(jsErrors).toEqual([]);
  });
});

test.describe("JSPI blocking cells (PRD-0043)", () => {
  test("blocking sleep suspends and blocking fetch returns same-origin content", async ({
    page,
  }) => {
    test.setTimeout(180_000);
    const jsErrors = trackJsErrors(page);

    // The feature needs JSPI (Chrome/Edge 137+). The bundled Chromium must
    // have it, otherwise this coverage is silently meaningless — fail loudly.
    await page.goto("/");
    const jspiAvailable = await page.evaluate(
      () =>
        typeof (WebAssembly as any).Suspending === "function" &&
        typeof (WebAssembly as any).promising === "function"
    );
    expect(
      jspiAvailable,
      "bundled Chromium lacks JSPI; upgrade Playwright browsers"
    ).toBe(true);

    const cell = await newNotebookWithCell(page);
    // Fully synchronous cell code: no .await anywhere. The sleep proves the
    // stack actually suspends (elapsed wall time), the fetch proves the
    // two-phase payload protocol against a same-origin static asset.
    await setCellSource(
      page,
      cell,
      [
        "let sw = Stopwatch::new();",
        "blocking::sleep_ms(300.0);",
        "let elapsed = sw.elapsed_ms();",
        'let body = blocking::fetch_text("/notebooks/welcome.ironpad");',
        "let fetched = body.map(|b| b.len()).unwrap_or(0);",
        'CellOutput::text(format!("slept_ok={} fetched_ok={}", elapsed >= 250.0, fetched > 100))',
      ].join("\n")
    );

    await runCellToSuccess(page, cell);

    const outputText = cell.locator(".ironpad-output-display-text");
    await expect(outputText).toBeVisible({ timeout: 5_000 });
    await expect(outputText).toContainText("slept_ok=true fetched_ok=true");

    expect(jsErrors).toEqual([]);
  });
});
