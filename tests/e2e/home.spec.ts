import { test, expect } from "@playwright/test";

test.describe("Home page", () => {
  test("loads and displays ironpad branding", async ({ page }) => {
    // Collect JS errors during navigation.
    const jsErrors: string[] = [];
    page.on("pageerror", (error) => jsErrors.push(error.message));

    // Navigate to home page.
    const response = await page.goto("/");
    expect(response).not.toBeNull();
    expect(response!.status()).toBe(200);

    // Verify page title contains "ironpad".
    await expect(page).toHaveTitle(/ironpad/i);

    // Verify the brand link is visible in the header.
    const brand = page.locator("a.ironpad-brand");
    await expect(brand).toBeVisible();
    await expect(brand).toHaveText("ironpad");

    // Verify the home page content area rendered.
    const home = page.locator(".ironpad-home");
    await expect(home).toBeVisible();

    // Verify no JS errors occurred.
    expect(jsErrors).toEqual([]);
  });
});
