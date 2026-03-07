import { test, expect } from "@playwright/test";

test.describe("Notebook", () => {
  test("create notebook and add cell", async ({ page }) => {
    // Collect JS errors during the test (filter known WASM hydration noise).
    const jsErrors: string[] = [];
    page.on("pageerror", (error) => {
      if (!error.message.includes("unreachable")) {
        jsErrors.push(error.message);
      }
    });

    // Navigate to home page.
    await page.goto("/");
    await expect(page.locator(".ironpad-home")).toBeVisible();

    // Click "+ New Notebook" button.
    await page.locator("button", { hasText: "+ New Notebook" }).click();

    // Verify navigation to /notebook/{id}.
    await expect(page).toHaveURL(/\/notebook\/[a-f0-9-]+/);

    // Verify the notebook editor is visible.
    await expect(page.locator(".ironpad-editor")).toBeVisible();

    // New notebooks start empty; verify the cell list is present.
    const cells = page.locator(".ironpad-cell-card");
    await expect(cells).toHaveCount(0);

    // Click "+ Add Cell" to add the first cell.
    await page.locator(".ironpad-add-cell-btn").first().click();

    // Verify a cell editor is now visible.
    await expect(cells).toHaveCount(1);
    await expect(page.locator(".ironpad-cell-editor-pane").first()).toBeVisible();

    // Click "+ Add Cell" again to add a second cell.
    await page.locator(".ironpad-add-cell-btn").last().click();

    // Verify two cells now exist.
    await expect(cells).toHaveCount(2);

    // Verify no JS errors occurred.
    expect(jsErrors).toEqual([]);
  });

  test("compile and execute a trivial cell", async ({ page }) => {
    // Compilation can take a while (cold cargo build to WASM).
    test.setTimeout(180_000);

    // Collect JS errors (filter known WASM hydration noise).
    const jsErrors: string[] = [];
    page.on("pageerror", (error) => {
      if (!error.message.includes("unreachable")) {
        jsErrors.push(error.message);
      }
    });

    // Create a new notebook.
    await page.goto("/");
    await expect(page.locator(".ironpad-home")).toBeVisible();
    await page.locator("button", { hasText: "+ New Notebook" }).click();
    await expect(page).toHaveURL(/\/notebook\/[a-f0-9-]+/);
    await expect(page.locator(".ironpad-editor")).toBeVisible();

    // Add a cell (default code: CellOutput::text("hello from ironpad").into()).
    await page.locator(".ironpad-add-cell-btn").first().click();
    const cell = page.locator(".ironpad-cell-card").first();
    await expect(cell).toBeVisible();

    // Click the run button ("▶") to compile and execute.
    const runButton = cell.locator(".ironpad-cell-actions button").first();
    await expect(runButton).toBeVisible();
    await runButton.click();

    // Wait for compilation to start.
    await expect(cell.locator(".ironpad-cell-status--compiling")).toBeVisible({
      timeout: 5_000,
    });

    // Wait for compilation to finish (status leaves "compiling").
    await expect(cell.locator(".ironpad-cell-status--compiling")).toBeHidden({
      timeout: 120_000,
    });

    // Verify the cell reached success status.
    await expect(cell.locator(".ironpad-cell-status--success")).toBeVisible({
      timeout: 5_000,
    });

    // Verify the output panel appeared and contains the expected text.
    const outputText = cell.locator(".ironpad-output-display-text");
    await expect(outputText).toBeVisible({ timeout: 5_000 });
    await expect(outputText).toContainText("hello from ironpad");

    // Verify no JS errors occurred.
    expect(jsErrors).toEqual([]);
  });

  test("two-cell data flow via bincode", async ({ page }) => {
    // Two compilations back-to-back — generous timeout.
    test.setTimeout(300_000);

    // Collect JS errors (filter known WASM hydration noise).
    const jsErrors: string[] = [];
    page.on("pageerror", (error) => {
      if (!error.message.includes("unreachable")) {
        jsErrors.push(error.message);
      }
    });

    const fs = await import("fs");
    const path = await import("path");

    // ── Create a new notebook ───────────────────────────────────────────
    await page.goto("/");
    await expect(page.locator(".ironpad-home")).toBeVisible();
    await page.locator("button", { hasText: "+ New Notebook" }).click();
    await expect(page).toHaveURL(/\/notebook\/[a-f0-9-]+/);
    await expect(page.locator(".ironpad-editor")).toBeVisible();

    // Extract notebook ID from URL.
    const notebookId = page.url().split("/notebook/")[1];

    // ── Add Cell 0 (producer) ───────────────────────────────────────────
    await page.locator(".ironpad-add-cell-btn").first().click();
    const cell0 = page.locator(".ironpad-cell-card").nth(0);
    await expect(cell0).toBeVisible();

    // ── Add Cell 1 (consumer) ───────────────────────────────────────────
    await page.locator(".ironpad-add-cell-btn").last().click();
    const cell1 = page.locator(".ironpad-cell-card").nth(1);
    await expect(cell1).toBeVisible();

    // Wait briefly for cells to persist to disk.
    await page.waitForTimeout(1_000);

    // ── Inject cell source via filesystem ───────────────────────────────
    // Read the notebook manifest to discover cell IDs.
    const dataDir = path.join(process.cwd(), "data", "notebooks", notebookId);
    const manifest = JSON.parse(
      fs.readFileSync(path.join(dataDir, "ironpad.json"), "utf-8")
    );
    const cellIds = manifest.cells.map((c: { id: string }) => c.id);
    expect(cellIds.length).toBe(2);

    // Cell 0: serialize Vec<i32> with display text.
    const cell0Source = [
      '    let data: Vec<i32> = vec![1, 2, 3, 4, 5];',
      '    CellOutput::new(&data).unwrap().with_display(format!("Sent: {:?}", data)).into()',
    ].join("\n");
    fs.writeFileSync(
      path.join(dataDir, "cells", cellIds[0], "source.rs"),
      cell0Source
    );

    // Cell 1: deserialize Vec<i32> from Cell 0 and compute sum.
    const cell1Source = [
      "    let input = CellInput::new(unsafe { std::slice::from_raw_parts(input_ptr, input_len) });",
      "    let data: Vec<i32> = input.deserialize().unwrap();",
      "    let sum: i32 = data.iter().sum();",
      '    CellOutput::text(format!("Sum: {}", sum)).into()',
    ].join("\n");
    fs.writeFileSync(
      path.join(dataDir, "cells", cellIds[1], "source.rs"),
      cell1Source
    );

    // ── Reload the page to pick up new source from server ───────────────
    await page.reload();
    await expect(page.locator(".ironpad-editor")).toBeVisible();

    // Re-query cell cards after reload.
    const cells = page.locator(".ironpad-cell-card");
    await expect(cells).toHaveCount(2, { timeout: 10_000 });
    const c0 = cells.nth(0);
    const c1 = cells.nth(1);

    // Wait for Monaco editors to mount in both cells.
    await expect(c0.locator(".monaco-editor").first()).toBeVisible({
      timeout: 15_000,
    });
    await expect(c1.locator(".monaco-editor").first()).toBeVisible({
      timeout: 15_000,
    });

    // ── Run all cells via Ctrl+Shift+Enter ──────────────────────────────
    await page.keyboard.press("Control+Shift+Enter");

    // ── Wait for Cell 0 to compile and succeed ──────────────────────────
    await expect(c0.locator(".ironpad-cell-status--compiling")).toBeVisible({
      timeout: 10_000,
    });
    await expect(c0.locator(".ironpad-cell-status--compiling")).toBeHidden({
      timeout: 120_000,
    });
    await expect(c0.locator(".ironpad-cell-status--success")).toBeVisible({
      timeout: 10_000,
    });

    // Verify Cell 0 output shows the sent data.
    const cell0Output = c0.locator(".ironpad-output-display-text");
    await expect(cell0Output).toBeVisible({ timeout: 5_000 });
    await expect(cell0Output).toContainText("Sent: [1, 2, 3, 4, 5]");

    // ── Wait for Cell 1 to compile and succeed ──────────────────────────
    await expect(
      c1.locator(
        ".ironpad-cell-status--compiling, .ironpad-cell-status--running, .ironpad-cell-status--success"
      )
    ).toBeVisible({ timeout: 30_000 });
    await expect(
      c1.locator(".ironpad-cell-status--compiling")
    ).toBeHidden({ timeout: 120_000 });
    await expect(
      c1.locator(".ironpad-cell-status--running")
    ).toBeHidden({ timeout: 10_000 });
    await expect(c1.locator(".ironpad-cell-status--success")).toBeVisible({
      timeout: 10_000,
    });

    // ── Verify Cell 1 output shows the expected sum ─────────────────────
    const cell1Output = c1.locator(".ironpad-output-display-text");
    await expect(cell1Output).toBeVisible({ timeout: 5_000 });
    await expect(cell1Output).toContainText("Sum: 15");

    // Verify no JS errors occurred.
    expect(jsErrors).toEqual([]);
  });
});
