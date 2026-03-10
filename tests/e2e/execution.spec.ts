import { test, expect } from "@playwright/test";

test.describe("Cell execution and output", () => {
  test("cell returning integer displays output", async ({ page }) => {
    // Compilation can take a while (cold cargo build to WASM).
    test.setTimeout(180_000);

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

    const notebookId = page.url().split("/notebook/")[1];

    // ── Add a cell ──────────────────────────────────────────────────────
    await page.locator(".ironpad-add-cell-btn").first().click();
    const cell = page.locator(".ironpad-cell-card").first();
    await expect(cell).toBeVisible();

    // Wait for the cell to persist to disk.
    await page.waitForTimeout(1_000);

    // ── Inject cell source via filesystem ────────────────────────────────
    const dataDir = path.join(process.cwd(), "data", "notebooks", notebookId);
    const manifest = JSON.parse(
      fs.readFileSync(path.join(dataDir, "ironpad.json"), "utf-8")
    );
    const cellId = manifest.cells[0].id;

    const cellSource = '    CellOutput::text(format!("{}", 42)).into()';
    fs.writeFileSync(
      path.join(dataDir, "cells", cellId, "source.rs"),
      cellSource
    );

    // Reload to pick up the new source.
    await page.reload();
    await expect(page.locator(".ironpad-editor")).toBeVisible();

    const reloadedCell = page.locator(".ironpad-cell-card").first();
    await expect(reloadedCell).toBeVisible({ timeout: 10_000 });
    await expect(
      reloadedCell.locator(".monaco-editor").first()
    ).toBeVisible({ timeout: 15_000 });

    // ── Run the cell ────────────────────────────────────────────────────
    const runButton = reloadedCell
      .locator(".ironpad-cell-actions button")
      .first();
    await expect(runButton).toBeVisible();
    await runButton.click();

    // Wait for compilation to start then finish.
    await expect(
      reloadedCell.locator(".ironpad-cell-status--compiling")
    ).toBeVisible({ timeout: 5_000 });
    await expect(
      reloadedCell.locator(".ironpad-cell-status--compiling")
    ).toBeHidden({ timeout: 120_000 });

    // Verify the cell reached success status.
    await expect(
      reloadedCell.locator(".ironpad-cell-status--success")
    ).toBeVisible({ timeout: 5_000 });

    // Verify the output contains "42".
    const outputText = reloadedCell.locator(".ironpad-output-display-text");
    await expect(outputText).toBeVisible({ timeout: 5_000 });
    await expect(outputText).toContainText("42");

    // Verify no JS errors occurred.
    expect(jsErrors).toEqual([]);
  });

  test("cell returning string displays output", async ({ page }) => {
    // Compilation can take a while.
    test.setTimeout(180_000);

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

    const notebookId = page.url().split("/notebook/")[1];

    // ── Add a cell ──────────────────────────────────────────────────────
    await page.locator(".ironpad-add-cell-btn").first().click();
    const cell = page.locator(".ironpad-cell-card").first();
    await expect(cell).toBeVisible();

    // Wait for the cell to persist to disk.
    await page.waitForTimeout(1_000);

    // ── Inject cell source via filesystem ────────────────────────────────
    const dataDir = path.join(process.cwd(), "data", "notebooks", notebookId);
    const manifest = JSON.parse(
      fs.readFileSync(path.join(dataDir, "ironpad.json"), "utf-8")
    );
    const cellId = manifest.cells[0].id;

    const cellSource =
      '    CellOutput::text("ironpad rocks".to_string()).into()';
    fs.writeFileSync(
      path.join(dataDir, "cells", cellId, "source.rs"),
      cellSource
    );

    // Reload to pick up the new source.
    await page.reload();
    await expect(page.locator(".ironpad-editor")).toBeVisible();

    const reloadedCell = page.locator(".ironpad-cell-card").first();
    await expect(reloadedCell).toBeVisible({ timeout: 10_000 });
    await expect(
      reloadedCell.locator(".monaco-editor").first()
    ).toBeVisible({ timeout: 15_000 });

    // ── Run the cell ────────────────────────────────────────────────────
    const runButton = reloadedCell
      .locator(".ironpad-cell-actions button")
      .first();
    await expect(runButton).toBeVisible();
    await runButton.click();

    // Wait for compilation to start then finish.
    await expect(
      reloadedCell.locator(".ironpad-cell-status--compiling")
    ).toBeVisible({ timeout: 5_000 });
    await expect(
      reloadedCell.locator(".ironpad-cell-status--compiling")
    ).toBeHidden({ timeout: 120_000 });

    // Verify the cell reached success status.
    await expect(
      reloadedCell.locator(".ironpad-cell-status--success")
    ).toBeVisible({ timeout: 5_000 });

    // Verify the output contains the expected string.
    const outputText = reloadedCell.locator(".ironpad-output-display-text");
    await expect(outputText).toBeVisible({ timeout: 5_000 });
    await expect(outputText).toContainText("ironpad rocks");

    // Verify no JS errors occurred.
    expect(jsErrors).toEqual([]);
  });
});
