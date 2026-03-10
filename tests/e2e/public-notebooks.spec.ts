import { test, expect } from "@playwright/test";

test.describe("Public notebooks", () => {
  test("home page shows public notebook badges", async ({ page }) => {
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

    // Verify at least one public notebook badge is visible.
    const publicBadge = page.locator(".ironpad-notebook-badge.public");
    await expect(publicBadge.first()).toBeVisible({ timeout: 10_000 });

    // Verify no JS errors occurred.
    expect(jsErrors).toEqual([]);
  });

  test("public notebook loads with cells and fork button", async ({
    page,
  }) => {
    // Collect JS errors (filter known WASM hydration noise).
    const jsErrors: string[] = [];
    page.on("pageerror", (error) => {
      if (!error.message.includes("unreachable")) {
        jsErrors.push(error.message);
      }
    });

    // Navigate directly to the Welcome public notebook.
    await page.goto("/notebook/public/welcome.ironpad");

    // Verify the view-only notebook container is visible.
    await expect(page.locator(".view-only-notebook")).toBeVisible({
      timeout: 15_000,
    });

    // Verify cells are rendered.
    const cells = page.locator(".view-only-cell");
    await expect(cells.first()).toBeVisible({ timeout: 10_000 });
    const count = await cells.count();
    expect(count).toBeGreaterThanOrEqual(1);

    // Verify the "Fork to Private" button is present.
    const forkButton = page.locator(".fork-button");
    await expect(forkButton).toBeVisible();
    await expect(forkButton).toContainText("Fork to Private");

    // Verify no JS errors occurred.
    expect(jsErrors).toEqual([]);
  });

  test("fork button navigates to new private notebook", async ({ page }) => {
    // Forking may take a moment — generous timeout.
    test.setTimeout(60_000);

    // Collect JS errors (filter known WASM hydration noise).
    const jsErrors: string[] = [];
    page.on("pageerror", (error) => {
      if (!error.message.includes("unreachable")) {
        jsErrors.push(error.message);
      }
    });

    // Navigate to the Welcome public notebook.
    await page.goto("/notebook/public/welcome.ironpad");
    await expect(page.locator(".view-only-notebook")).toBeVisible({
      timeout: 15_000,
    });

    // Click the fork button.
    const forkButton = page.locator(".fork-button");
    await expect(forkButton).toBeVisible();
    await forkButton.click();

    // Verify navigation to a new private notebook editor.
    await expect(page).toHaveURL(/\/notebook\/[a-f0-9-]+/, {
      timeout: 15_000,
    });
    await expect(page.locator(".ironpad-editor")).toBeVisible({
      timeout: 15_000,
    });

    // Verify the forked notebook has cells.
    const cells = page.locator(".ironpad-cell-card");
    await expect(cells.first()).toBeVisible({ timeout: 10_000 });

    // Verify no JS errors occurred.
    expect(jsErrors).toEqual([]);
  });
});
