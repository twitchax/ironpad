import { test, expect } from "@playwright/test";
import { trackJsErrors } from "./helpers/errors";

test.describe("Welcome public notebook", () => {
  test("welcome notebook is listed on home page and opens", async ({ page }) => {
    // Collect JS errors during navigation.
    const jsErrors = trackJsErrors(page);

    // Navigate to home page.
    await page.goto("/");
    await expect(page.locator(".ironpad-home")).toBeVisible();

    // Verify the "Welcome to ironpad" public notebook card is present.
    const welcomeCard = page
      .locator(".ironpad-notebook-card-link")
      .filter({ has: page.locator(".ironpad-notebook-card-title", { hasText: "Welcome to ironpad" }) });
    await expect(welcomeCard).toBeVisible({ timeout: 10_000 });

    // Verify the card shows 5 cells: four Fibonacci code cells plus the
    // leading AI-authorship banner every public notebook carries.
    const cellCount = welcomeCard.locator(".ironpad-notebook-card-cells");
    await expect(cellCount).toHaveText("5 cells");

    // Click into the notebook — it's a public notebook, so it opens in view-only mode.
    await welcomeCard.click();
    await expect(page).toHaveURL(/\/public\/welcome/);
    await expect(page.locator(".view-only-notebook")).toBeVisible({
      timeout: 15_000,
    });

    // Verify cells are rendered in view-only mode.
    const cells = page.locator(".view-only-cell");
    await expect(cells.first()).toBeVisible({ timeout: 10_000 });
    const count = await cells.count();
    expect(count).toBeGreaterThanOrEqual(4);

    // Verify no JS errors occurred.
    expect(jsErrors).toEqual([]);
  });
});
