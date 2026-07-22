import { test, expect } from "@playwright/test";
import { createNotebook } from "./helpers/session";

/**
 * PRD-0048: canonical routes — /local/{uuid}, /public/{name} (extension-less),
 * /shared/{hash} — with legacy /notebook/* URLs redirecting forever. Legacy
 * gotos scattered across other specs double as implicit redirect coverage;
 * these tests pin the URL-bar outcome explicitly.
 */

test.describe("Canonical routes (PRD-0048)", () => {
  test("legacy public URL redirects to extension-less /public", async ({
    page,
  }) => {
    await page.goto("/notebook/public/welcome.ironpad");
    await expect(page).toHaveURL(/\/public\/welcome$/, { timeout: 15_000 });
    await expect(page.locator(".view-only-notebook")).toBeVisible({
      timeout: 30_000,
    });
  });

  test("canonical /public/{name} renders directly", async ({ page }) => {
    await page.goto("/public/welcome");
    await expect(page).toHaveURL(/\/public\/welcome$/);
    await expect(page.locator(".view-only-notebook")).toBeVisible({
      timeout: 30_000,
    });
  });

  test("legacy /notebook/{id} redirects to /local/{id}", async ({ page }) => {
    await createNotebook(page);
    const id = page.url().match(/\/local\/([a-f0-9-]+)/)![1];

    await page.goto(`/notebook/${id}`);
    await expect(page).toHaveURL(new RegExp(`/local/${id}$`), {
      timeout: 15_000,
    });
    await expect(page.locator(".ironpad-editor")).toBeVisible({
      timeout: 15_000,
    });
  });
});
