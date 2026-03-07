import { test, expect } from "@playwright/test";

test.describe("Sample notebook seed", () => {
  test("sample notebook is pre-loaded on first run", async ({ page }) => {
    // Collect JS errors during navigation.
    const jsErrors: string[] = [];
    page.on("pageerror", (error) => {
      if (!error.message.includes("unreachable")) {
        jsErrors.push(error.message);
      }
    });

    // Navigate to home page.
    await page.goto("/");
    await expect(page.locator(".ironpad-home")).toBeVisible();

    // Verify the "Welcome to ironpad" notebook card is present.
    const welcomeCard = page
      .locator(".ironpad-notebook-card-link")
      .filter({ has: page.locator(".ironpad-notebook-card-title", { hasText: "Welcome to ironpad" }) });
    await expect(welcomeCard).toBeVisible({ timeout: 10_000 });

    // Verify the card shows 2 cells.
    const cellCount = welcomeCard.locator(".ironpad-notebook-card-cells");
    await expect(cellCount).toHaveText("2 cells");

    // Click into the notebook and verify cells are present.
    await welcomeCard.click();
    await expect(page).toHaveURL(/\/notebook\/[a-f0-9-]+/);
    await expect(page.locator(".ironpad-editor")).toBeVisible();

    // Verify 2 cell cards exist.
    const cells = page.locator(".ironpad-cell-card");
    await expect(cells).toHaveCount(2, { timeout: 10_000 });

    // Verify no JS errors occurred.
    expect(jsErrors).toEqual([]);
  });
});
