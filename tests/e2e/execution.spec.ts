import { test, expect, type Page } from "@playwright/test";
import { setCellSource } from "./helpers/monaco";
import { trackJsErrors } from "./helpers/errors";
import { ADD_CODE, createNotebook } from "./helpers/session";

/** A fresh notebook with one Code cell per source; returns the cell cards. */
async function newNotebookWithCells(page: Page, sources: string[]) {
  await createNotebook(page);
  for (let i = 0; i < sources.length; i += 1) {
    await page.locator(ADD_CODE).last().click();
    const cell = page.locator(".ironpad-cell-card").nth(i);
    await expect(cell.locator(".monaco-editor").first()).toBeVisible({
      timeout: 15_000,
    });
    await setCellSource(page, cell, sources[i]);
  }
  return page.locator(".ironpad-cell-card");
}

test.describe("Cell execution and output", () => {
  test("cell returning integer displays output", async ({ page }) => {
    test.setTimeout(180_000);

    const jsErrors = trackJsErrors(page);

    // Create a new notebook and add a cell.
    await page.goto("/");
    await expect(page.locator(".ironpad-home")).toBeVisible();
    await page.waitForTimeout(3_000); // hydration (suite convention)
    await page.locator("button", { hasText: "New Notebook" }).click();
    await expect(page).toHaveURL(/\/local\/[a-f0-9-]+/);
    await expect(page.locator(".ironpad-editor")).toBeVisible();

    await page.locator(".ironpad-add-cell-btn").first().click();
    const cell = page.locator(".ironpad-cell-card").first();
    await expect(cell).toBeVisible();
    await expect(cell.locator(".monaco-editor").first()).toBeVisible({
      timeout: 15_000,
    });

    // Set cell source via Monaco API.
    // The scaffold wraps this in `({ source }).into()`, so we provide a
    // bare expression that converts to CellOutput.
    await setCellSource(page, cell, 'CellOutput::text(format!("{}", 42))');

    // Run the cell.
    const runButton = page.locator('button[title="Run cell"]').first();
    await expect(runButton).toBeVisible();
    await runButton.click();

    // Wait for compilation.
    // Assert the TERMINAL state: "compiling" is transient and a warm
    // cache can skip past it between polls.
    await expect(cell.locator(".ironpad-cell-status--success")).toBeVisible({
      timeout: 120_000,
    });

    const outputText = cell.locator(".ironpad-output-display-text");
    await expect(outputText).toBeVisible({ timeout: 5_000 });
    await expect(outputText).toContainText("42");

    expect(jsErrors).toEqual([]);
  });

  test("cell returning string displays output", async ({ page }) => {
    test.setTimeout(180_000);

    const jsErrors = trackJsErrors(page);

    // Create a new notebook and add a cell.
    await page.goto("/");
    await expect(page.locator(".ironpad-home")).toBeVisible();
    await page.waitForTimeout(3_000); // hydration (suite convention)
    await page.locator("button", { hasText: "New Notebook" }).click();
    await expect(page).toHaveURL(/\/local\/[a-f0-9-]+/);
    await expect(page.locator(".ironpad-editor")).toBeVisible();

    await page.locator(".ironpad-add-cell-btn").first().click();
    const cell = page.locator(".ironpad-cell-card").first();
    await expect(cell).toBeVisible();
    await expect(cell.locator(".monaco-editor").first()).toBeVisible({
      timeout: 15_000,
    });

    // Set cell source via Monaco API.
    await setCellSource(
      page,
      cell,
      'CellOutput::text("ironpad rocks".to_string())'
    );

    // Run the cell.
    const runButton = page.locator('button[title="Run cell"]').first();
    await expect(runButton).toBeVisible();
    await runButton.click();

    // Wait for compilation.
    // Assert the TERMINAL state: "compiling" is transient and a warm
    // cache can skip past it between polls.
    await expect(cell.locator(".ironpad-cell-status--success")).toBeVisible({
      timeout: 120_000,
    });

    const outputText = cell.locator(".ironpad-output-display-text");
    await expect(outputText).toBeVisible({ timeout: 5_000 });
    await expect(outputText).toContainText("ironpad rocks");

    expect(jsErrors).toEqual([]);
  });

  test("plot cell with text renders in the worker without main-thread fallback", async ({
    page,
  }) => {
    test.setTimeout(180_000);

    const jsErrors = trackJsErrors(page);

    // Create a new notebook and add a cell.
    await page.goto("/");
    await expect(page.locator(".ironpad-home")).toBeVisible();
    await page.waitForTimeout(3_000); // hydration (suite convention)
    await page.locator("button", { hasText: "New Notebook" }).click();
    await expect(page).toHaveURL(/\/local\/[a-f0-9-]+/);
    await expect(page.locator(".ironpad-editor")).toBeVisible();

    await page.locator(".ironpad-add-cell-btn").first().click();
    const cell = page.locator(".ironpad-cell-card").first();
    await expect(cell).toBeVisible();
    await expect(cell.locator(".monaco-editor").first()).toBeVisible({
      timeout: 15_000,
    });

    // A plot with a title and axis labels forces plotters to lay out text.
    // On wasm32 plotters measures text through the DOM, which does not exist
    // in the executor worker — the worker's DOM shim must satisfy it, or the
    // cell panics and bounces to the slow main-thread fallback.
    await setCellSource(
      page,
      cell,
      'let data: Vec<(f64, f64)> = (0..=10).map(|i| (i as f64, (i * i) as f64)).collect();\n' +
        "Plot::scatter(&data)\n" +
        '    .title("Squares")\n' +
        '    .x_label("x")\n' +
        '    .y_label("x squared")'
    );

    // Run the cell.
    const runButton = page.locator('button[title="Run cell"]').first();
    await expect(runButton).toBeVisible();
    await runButton.click();

    // Wait for compilation.
    // Assert the TERMINAL state: "compiling" is transient and a warm
    // cache can skip past it between polls.
    await expect(cell.locator(".ironpad-cell-status--success")).toBeVisible({
      timeout: 120_000,
    });

    // The SVG chart must render...
    // :not(.ironpad-icon) — the copy button inside the panel is an inline
    // SVG icon too, so a bare `svg` matches two elements (PRD-0062).
    const svg = cell.locator(".ironpad-output-svg svg:not(.ironpad-icon)");
    await expect(svg).toBeVisible({ timeout: 5_000 });

    // ...from the WORKER: no "⚠ main thread" fallback badge. This is the
    // regression assertion for the DOM-measurement shim in executor-worker.js.
    await expect(cell.locator(".ironpad-output-fallback-badge")).toHaveCount(0);

    expect(jsErrors).toEqual([]);
  });

  test("a rayon cell's host message crosses from the worker without a fallback", async ({
    page,
  }) => {
    // Regression (review js-1): a rayon cell's memory is a SharedArrayBuffer,
    // and the worker used to decode each host message from a LIVE view of it.
    // A browser TextDecoder rejects shared views, so the throw landed inside
    // the WASM import, trapped the cell, and it silently re-ran on the main
    // thread. The host message here is a sim_emit, whose arrival on the
    // bridge's own bus is the positive control that the forward happened.
    // A cold rayon build rebuilds std with atomics, hence the long timeout.
    test.setTimeout(300_000);

    const jsErrors = trackJsErrors(page);

    await page.goto("/");
    await expect(page.locator(".ironpad-home")).toBeVisible();
    await page.waitForTimeout(3_000); // hydration (suite convention)
    await page.locator("button", { hasText: "New Notebook" }).click();
    await expect(page).toHaveURL(/\/local\/[a-f0-9-]+/);
    await expect(page.locator(".ironpad-editor")).toBeVisible();

    await page.locator(".ironpad-add-cell-btn").first().click();
    const cell = page.locator(".ironpad-cell-card").first();
    await expect(cell).toBeVisible();
    await expect(cell.locator(".monaco-editor").first()).toBeVisible({
      timeout: 15_000,
    });

    // Each tab owns its own Monaco editor pane: [0] Code, [1] Cargo.toml.
    const panes = cell.locator(".ironpad-cell-editor-pane");
    await cell.locator(".ironpad-cell-tab", { hasText: "Cargo.toml" }).click();
    await expect(panes.nth(1).locator(".monaco-editor")).toBeVisible({
      timeout: 15_000,
    });
    await setCellSource(page, panes.nth(1), '[dependencies]\nrayon = "1"\n');
    await cell.locator(".ironpad-cell-tab", { hasText: "Code" }).click();
    await setCellSource(
      page,
      panes.nth(0),
      "let total: u64 = (1..=1000u64).into_par_iter().sum();\n" +
        'sim::emit("e2e-rayon-sum", &total);\n' +
        'CellOutput::text(format!("sum={total}"))'
    );

    await page.locator('button[title="Run cell"]').first().click();
    await expect(cell.locator(".ironpad-cell-status--success")).toBeVisible({
      timeout: 240_000,
    });
    await expect(cell.locator(".ironpad-output-display-text")).toContainText(
      "sum=500500"
    );

    // The message reached the main thread...
    const forwarded = await page.evaluate(() =>
      (window as any).IronpadExecutor.simBusRead("e2e-rayon-sum")
    );
    expect(forwarded).toBe(500500);

    // ...and the cell ran in the WORKER, not on the main-thread fallback.
    await expect(cell.locator(".ironpad-output-fallback-badge")).toHaveCount(0);

    expect(jsErrors).toEqual([]);
  });

  test("a panicking cell reports its panic message, not a memory diagnosis", async ({
    page,
  }) => {
    // Regression (review bug-js-2): the worker captured the panic text, then
    // the main-thread retry hit a bare `unreachable` with nothing patching
    // console.error there, and its "ran out of memory" sentence replaced it.
    test.setTimeout(180_000);
    const jsErrors = trackJsErrors(page);

    const cells = await newNotebookWithCells(page, [
      [
        "let rows: Vec<i32> = Vec::new();",
        'if rows.is_empty() { panic!("kaboom-123"); }',
        'CellOutput::text(format!("{}", rows.len()))',
      ].join("\n"),
    ]);
    await page.locator('button[title="Run cell"]').first().click();

    const cell = cells.first();
    await expect(cell.locator(".ironpad-cell-status--error")).toBeVisible({
      timeout: 120_000,
    });
    const output = cell.locator(".ironpad-output-panel");
    await expect(output).toContainText("kaboom-123");
    await expect(output).not.toContainText("ran out of memory");

    expect(jsErrors).toEqual([]);
  });

  test("an async cell that panics fails cleanly and leaves the worker alive", async ({
    page,
  }) => {
    // Regression (review bug-js-3): the panic traps inside a
    // wasm-bindgen-futures microtask, so it never rejected cell_main's
    // promise. In the worker it crashed the worker; the main-thread retry
    // then never settled, and the cell sat on Running with Stop unable to
    // reach it, holding up every cell after it.
    test.setTimeout(240_000);
    const jsErrors = trackJsErrors(page);

    const cells = await newNotebookWithCells(page, [
      [
        'async fn fetch_rows() -> Vec<i32> { panic!("async-kaboom-456") }',
        "let rows = fetch_rows().await;",
        'CellOutput::text(format!("{}", rows.len()))',
      ].join("\n"),
      'CellOutput::text("after the panic".to_string())',
    ]);
    const hasWorker = await page.evaluate(() => {
      const w = (window as any).IronpadExecutor._worker;
      (window as any).__e2eWorkerBefore = w;
      return w !== null;
    });
    expect(hasWorker).toBe(true);

    await page.locator('button[title="Run cell"]').first().click();
    await expect(cells.nth(0).locator(".ironpad-cell-status--error")).toBeVisible({
      timeout: 120_000,
    });
    await expect(cells.nth(0).locator(".ironpad-output-panel")).toContainText(
      "async-kaboom-456"
    );

    await page.locator('button[title="Run cell"]').nth(1).click();
    await expect(cells.nth(1).locator(".ironpad-cell-status--success")).toBeVisible({
      timeout: 120_000,
    });

    const sameWorker = await page.evaluate(
      () => (window as any).IronpadExecutor._worker === (window as any).__e2eWorkerBefore
    );
    expect(sameWorker).toBe(true);

    expect(jsErrors).toEqual([]);
  });
});
