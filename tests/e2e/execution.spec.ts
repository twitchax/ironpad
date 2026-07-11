import { test, expect } from "@playwright/test";
import { setCellSource } from "./helpers/monaco";
import { trackJsErrors } from "./helpers/errors";

test.describe("Cell execution and output", () => {
  test("cell returning integer displays output", async ({ page }) => {
    test.setTimeout(180_000);

    const jsErrors = trackJsErrors(page);

    // Create a new notebook and add a cell.
    await page.goto("/");
    await expect(page.locator(".ironpad-home")).toBeVisible();
    await page.waitForTimeout(3_000); // hydration (suite convention)
    await page.locator("button", { hasText: "+ New Notebook" }).click();
    await expect(page).toHaveURL(/\/notebook\/[a-f0-9-]+/);
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
    await expect(cell.locator(".ironpad-cell-status--compiling")).toBeVisible({
      timeout: 5_000,
    });
    await expect(cell.locator(".ironpad-cell-status--compiling")).toBeHidden({
      timeout: 120_000,
    });

    await expect(cell.locator(".ironpad-cell-status--success")).toBeVisible({
      timeout: 5_000,
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
    await page.locator("button", { hasText: "+ New Notebook" }).click();
    await expect(page).toHaveURL(/\/notebook\/[a-f0-9-]+/);
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
    await expect(cell.locator(".ironpad-cell-status--compiling")).toBeVisible({
      timeout: 5_000,
    });
    await expect(cell.locator(".ironpad-cell-status--compiling")).toBeHidden({
      timeout: 120_000,
    });

    await expect(cell.locator(".ironpad-cell-status--success")).toBeVisible({
      timeout: 5_000,
    });

    const outputText = cell.locator(".ironpad-output-display-text");
    await expect(outputText).toBeVisible({ timeout: 5_000 });
    await expect(outputText).toContainText("ironpad rocks");

    expect(jsErrors).toEqual([]);
  });
});
