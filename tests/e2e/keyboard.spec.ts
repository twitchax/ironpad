import { test, expect } from "@playwright/test";

test.describe("Keyboard shortcuts", () => {
  test("Ctrl+Enter in cell editor starts compilation", async ({ page }) => {
    // Compilation can take a while (cold cargo build to WASM).
    test.setTimeout(180_000);

    // Collect JS errors (filter known WASM hydration noise).
    const jsErrors: string[] = [];
    page.on("pageerror", (error) => {
      if (!error.message.includes("unreachable")) {
        jsErrors.push(error.message);
      }
    });

    // ── Create a new notebook ───────────────────────────────────────────
    await page.goto("/");
    await expect(page.locator(".ironpad-home")).toBeVisible();
    await page.locator("button", { hasText: "+ New Notebook" }).click();
    await expect(page).toHaveURL(/\/notebook\/[a-f0-9-]+/);
    await expect(page.locator(".ironpad-editor")).toBeVisible();

    // ── Add a cell ──────────────────────────────────────────────────────
    await page.locator(".ironpad-add-cell-btn").first().click();
    const cell = page.locator(".ironpad-cell-card").first();
    await expect(cell).toBeVisible();

    // Wait for Monaco editor to mount.
    const monacoEditor = cell.locator(".monaco-editor").first();
    await expect(monacoEditor).toBeVisible({ timeout: 15_000 });

    // Click into the Monaco editor to give it focus.
    await monacoEditor.click();

    // ── Press Ctrl+Enter to trigger compilation ─────────────────────────
    await page.keyboard.press("Control+Enter");

    // Verify compilation starts (status indicator changes to compiling).
    await expect(
      cell.locator(".ironpad-cell-status--compiling")
    ).toBeVisible({ timeout: 10_000 });

    // Wait for compilation to finish.
    await expect(
      cell.locator(".ironpad-cell-status--compiling")
    ).toBeHidden({ timeout: 120_000 });

    // Verify the cell reached a terminal state (success or error).
    await expect(
      cell.locator(
        ".ironpad-cell-status--success, .ironpad-cell-status--error"
      )
    ).toBeVisible({ timeout: 5_000 });

    // Verify no JS errors occurred.
    expect(jsErrors).toEqual([]);
  });

  test("Ctrl+Shift+Enter runs all cells", async ({ page }) => {
    // Compilation can take a while.
    test.setTimeout(180_000);

    // Collect JS errors (filter known WASM hydration noise).
    const jsErrors: string[] = [];
    page.on("pageerror", (error) => {
      if (!error.message.includes("unreachable")) {
        jsErrors.push(error.message);
      }
    });

    // ── Create a new notebook with two cells ────────────────────────────
    await page.goto("/");
    await expect(page.locator(".ironpad-home")).toBeVisible();
    await page.locator("button", { hasText: "+ New Notebook" }).click();
    await expect(page).toHaveURL(/\/notebook\/[a-f0-9-]+/);
    await expect(page.locator(".ironpad-editor")).toBeVisible();

    // Add two cells.
    await page.locator(".ironpad-add-cell-btn").first().click();
    await expect(page.locator(".ironpad-cell-card")).toHaveCount(1);
    await page.locator(".ironpad-add-cell-btn").last().click();
    await expect(page.locator(".ironpad-cell-card")).toHaveCount(2);

    // Wait for Monaco editors to mount in both cells.
    const cells = page.locator(".ironpad-cell-card");
    await expect(cells.nth(0).locator(".monaco-editor").first()).toBeVisible({
      timeout: 15_000,
    });
    await expect(cells.nth(1).locator(".monaco-editor").first()).toBeVisible({
      timeout: 15_000,
    });

    // ── Press Ctrl+Shift+Enter to run all cells ─────────────────────────
    await page.keyboard.press("Control+Shift+Enter");

    // Verify at least one cell starts compiling.
    await expect(
      cells.nth(0).locator(".ironpad-cell-status--compiling")
    ).toBeVisible({ timeout: 10_000 });

    // Wait for the first cell to finish compilation.
    await expect(
      cells.nth(0).locator(".ironpad-cell-status--compiling")
    ).toBeHidden({ timeout: 120_000 });

    // Verify the first cell reached a terminal state.
    await expect(
      cells
        .nth(0)
        .locator(
          ".ironpad-cell-status--success, .ironpad-cell-status--error"
        )
    ).toBeVisible({ timeout: 5_000 });

    // Verify no JS errors occurred.
    expect(jsErrors).toEqual([]);
  });
});
