import { test, expect } from "@playwright/test";
import { setCellSource } from "./helpers/monaco";
import { ADD_MARKDOWN, createNotebook } from "./helpers/session";

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

test.describe("Per-page status bar state", () => {
  test("a read-only page reached from the editor does not inherit its Saved stamp", async ({
    page,
  }) => {
    // Every route resets the status surfaces it did not set. The editor's
    // "Saved: ..." used to survive a CLIENT-SIDE navigation onto /public,
    // which only reset the header title. A full page load cannot show this,
    // so the navigation goes through an in-app link.
    await createNotebook(page);
    await page.locator(ADD_MARKDOWN).first().click();
    const cell = page.locator(".ironpad-cell-card").first();
    await cell.locator(".ironpad-markdown-cell-preview").dblclick();
    await expect(cell.locator(".monaco-editor").first()).toBeVisible({ timeout: 15_000 });
    await setCellSource(page, cell, "[Welcome](/public/welcome)");

    await cell.locator(".ironpad-cell-header").click();
    await page.keyboard.press("Control+s");
    const bar = page.locator(".ironpad-status-bar");
    await expect(bar).toContainText("Saved:", { timeout: 10_000 });

    // Preview renders the markdown through the view-only renderer, whose
    // links the router intercepts: a client-side navigation.
    await page.locator('button[aria-label="Preview"]').click();
    await page.locator('.view-only-markdown a[href="/public/welcome"]').click();
    await expect(page).toHaveURL(/\/public\/welcome$/);
    await expect(page.locator(".view-only-notebook")).toBeVisible({ timeout: 30_000 });
    await expect(bar).toContainText("Cells:");
    await expect(bar).not.toContainText("Saved:");
  });
});
