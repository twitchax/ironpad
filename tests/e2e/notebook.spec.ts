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
});
