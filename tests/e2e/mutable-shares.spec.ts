import { test, expect, Page } from "@playwright/test";
import { createNotebook } from "./helpers/session";

/**
 * PRD-0049: mutable shares. Convert a private notebook to a server-backed
 * mutable share at /mutable/{id}, push updates, rebind on a fresh device with
 * a key, and unpublish. Keys are device-minted; the reader page is view-only
 * with an "enter your key" rebind control.
 */

const MENU = '.ironpad-toolbar-dropdown-toggle[title="Notebook menu"]';

/** Open the notebook (hamburger) menu and click an item by its label. */
async function menuClick(page: Page, label: string): Promise<void> {
  await page.locator(MENU).click();
  await page
    .locator(".ironpad-toolbar-dropdown-item", { hasText: label })
    .click();
}

/** Rename via the header title input and confirm the change landed. */
async function rename(page: Page, title: string): Promise<void> {
  await page.locator(".ironpad-notebook-title--editable").click();
  const input = page.locator(".ironpad-header-title-input");
  await expect(input).toBeVisible();
  await input.fill("");
  await input.pressSequentially(title, { delay: 15 });
  await input.press("Enter");
  await expect(page.locator(".ironpad-notebook-title--editable")).toHaveText(
    title,
    { timeout: 10_000 }
  );
}

/** Share Mutable and return the minted share id from the toast. */
async function shareMutable(page: Page): Promise<string> {
  await menuClick(page, "Share Mutable");
  const toast = page.locator(".ironpad-toast-body");
  await expect(toast).toContainText("/mutable/", { timeout: 30_000 });
  const text = (await toast.textContent())!;
  const id = text.match(/\/mutable\/([a-f0-9]{16})/);
  expect(id, `toast should carry a /mutable/{id} url: ${text}`).not.toBeNull();
  return id![1];
}

test.describe("Mutable shares (PRD-0049)", () => {
  test("convert to mutable, push an edit, and a fresh reader sees the update", async ({
    page,
    browser,
  }) => {
    test.setTimeout(90_000);
    await createNotebook(page);
    await page.waitForTimeout(1_500); // user key + binding load (hydration)

    await rename(page, "Mutable One");
    const shareId = await shareMutable(page);

    // The menu swaps Share Mutable → Push Update once bound.
    await page.locator(MENU).click();
    await expect(
      page.locator(".ironpad-toolbar-dropdown-item", { hasText: "Push Update" })
    ).toBeVisible();
    await expect(
      page.locator(".ironpad-toolbar-dropdown-item", {
        hasText: "Share Mutable",
      })
    ).toHaveCount(0);
    await page.locator(MENU).click(); // close

    // Edit, then Push the update.
    await rename(page, "Mutable One Edited");
    await menuClick(page, "Push Update");
    await expect(page.locator(".ironpad-toast-body")).toContainText("updated", {
      timeout: 30_000,
    });

    // A fresh context (no shared IndexedDB) reads the server copy.
    const ctx = await browser.newContext();
    try {
      const reader = await ctx.newPage();
      await reader.goto(`/mutable/${shareId}`);
      await expect(reader.locator(".view-only-notebook")).toBeVisible({
        timeout: 30_000,
      });
      await expect(reader.locator(".view-only-title")).toHaveText(
        "Mutable One Edited",
        { timeout: 15_000 }
      );
    } finally {
      await ctx.close();
    }
  });

  test("unpublish returns the notebook to the private list and 404s the link", async ({
    page,
  }) => {
    test.setTimeout(90_000);
    await createNotebook(page);
    await page.waitForTimeout(1_500);
    const notebookId = page.url().match(/\/local\/([a-f0-9-]+)/)![1];

    const shareId = await shareMutable(page);

    // Delete is replaced by Unpublish while mutable-backed.
    await page.locator(MENU).click();
    await expect(
      page.locator(".ironpad-toolbar-dropdown-item", { hasText: "Unpublish" })
    ).toBeVisible();
    page.on("dialog", (d) => d.accept());
    await page
      .locator(".ironpad-toolbar-dropdown-item", { hasText: "Unpublish" })
      .click();
    await expect(page.locator(".ironpad-toast-body")).toContainText(
      "private list",
      { timeout: 30_000 }
    );

    // The share is gone server-side.
    await page.goto(`/mutable/${shareId}`);
    await expect(page.locator(".ironpad-error-boundary-message")).toContainText(
      "not found",
      { timeout: 15_000 }
    );

    // And it's back as a private notebook on home.
    await page.goto("/");
    await expect(page.locator(".ironpad-home")).toBeVisible();
    await expect(
      page.locator(`a[href="/local/${notebookId}"]`)
    ).toBeVisible({ timeout: 10_000 });
  });

  test("rebind on a fresh context with the user key; a wrong key is rejected", async ({
    page,
    browser,
  }) => {
    test.setTimeout(90_000);
    await createNotebook(page);
    await page.waitForTimeout(1_500);

    const shareId = await shareMutable(page);

    // Read this device's user key from the status bar (either key authorizes).
    await page.locator('.ironpad-status-key-btn[title="Reveal"]').click();
    const userKey = (
      await page.locator(".ironpad-status-key-value").textContent()
    )!.trim();
    expect(userKey).toMatch(/^[a-f0-9]{64}$/);

    const ctx = await browser.newContext();
    try {
      const reader = await ctx.newPage();
      await reader.goto(`/mutable/${shareId}`);
      await expect(reader.locator(".view-only-notebook")).toBeVisible({
        timeout: 30_000,
      });
      await reader.waitForTimeout(2_500); // hydrate the rebind handler

      await reader.locator(".mutable-rebind-toggle").click();
      // Wrong key: valid format, no match.
      await reader.locator(".mutable-rebind-input").fill("0".repeat(64));
      await reader.locator(".mutable-rebind-submit").click();
      await expect(reader.locator(".mutable-rebind-status")).toContainText(
        "does not match",
        { timeout: 15_000 }
      );

      // Correct key: pulled into local storage, editor opens.
      await reader.locator(".mutable-rebind-input").fill(userKey);
      await reader.locator(".mutable-rebind-submit").click();
      await expect(reader).toHaveURL(/\/local\/[a-f0-9-]+/, { timeout: 15_000 });
      await expect(reader.locator(".ironpad-editor")).toBeVisible({
        timeout: 15_000,
      });

      // Push works from the rebound device.
      await reader.waitForTimeout(1_000); // binding load
      await menuClick(reader, "Push Update");
      await expect(reader.locator(".ironpad-toast-body")).toContainText(
        "updated",
        { timeout: 30_000 }
      );
    } finally {
      await ctx.close();
    }
  });
});
