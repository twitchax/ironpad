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
});
