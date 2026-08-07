import { test, expect } from "@playwright/test";
import { trackJsErrors } from "./helpers/errors";
import { createNotebook } from "./helpers/session";

test.describe("Home page", () => {
  test("loads and displays ironpad branding", async ({ page }) => {
    // Collect JS errors during navigation (shared filter for known noise).
    const jsErrors = trackJsErrors(page);

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

  // Private and public notebooks sit in one grid, so their badges must be
  // tellable apart. Both roles point at the same Lucide diamond and the
  // filled-vs-outline distinction lives entirely in CSS, which shipped
  // missing once: the two rendered byte-identical and the `◆` vs `◇`
  // reading the home page relied on was simply gone.
  test("private and public notebook badges are distinguishable", async ({
    page,
  }) => {
    test.setTimeout(60_000);

    // A private notebook to sit alongside the bundled public ones.
    await createNotebook(page);
    await page.goto("/");
    await expect(page.locator(".ironpad-home")).toBeVisible({
      timeout: 15_000,
    });

    const priv = page.locator(".ironpad-notebook-badge.private svg").first();
    const pub = page.locator(".ironpad-notebook-badge.public svg").first();
    await expect(priv).toBeVisible({ timeout: 15_000 });
    await expect(pub).toBeVisible({ timeout: 15_000 });

    // Same glyph is fine; identical PAINT is not.
    const fills = await Promise.all(
      [priv, pub].map((l) => l.evaluate((el) => getComputedStyle(el).fill))
    );
    expect(fills[0], "private badge should be filled").not.toBe("none");
    expect(fills[1], "public badge should be an outline").toBe("none");
  });
});
