import { expect, Page } from "@playwright/test";

/**
 * The notebook (hamburger) menu contract and the Share Mutable flow, shared
 * by every spec that drives them (mutable-shares, private-shares,
 * local-history, persisted-outputs). One home: a dropdown-class rename or a
 * change to the publish flow lands here once, instead of leaving a weaker
 * copy green against a broken flow.
 */

export const MENU = '.ironpad-toolbar-dropdown-toggle[title="Notebook menu"]';

/** Open the notebook (hamburger) menu and click an item by its label. */
export async function menuClick(page: Page, label: string): Promise<void> {
  await page.locator(MENU).click();
  await page
    .locator(".ironpad-toolbar-dropdown-item", { hasText: label })
    .click();
}

/** The owner's editor is mounted: cells UI + the Push button. */
export async function expectOwnerEditor(page: Page): Promise<void> {
  await expect(page.locator(".ironpad-push-button")).toBeVisible({
    timeout: 30_000,
  });
}

/**
 * Share Mutable: converts, deletes the local copy, and hard-navigates to
 * /mutable/{id} where the owner's editor mounts. Returns the minted id.
 */
export async function shareMutable(page: Page): Promise<string> {
  await menuClick(page, "Share Mutable");
  await expect(page).toHaveURL(/\/mutable\/[a-f0-9]{16}/, { timeout: 30_000 });
  const shareId = page.url().match(/\/mutable\/([a-f0-9]{16})/)![1];
  // The publish toast rides sessionStorage across the navigation.
  await expect(
    page.locator(".ironpad-toast-title", { hasText: "Published" }),
  ).toBeVisible({ timeout: 15_000 });
  // The owner lands in the editor (auto-swap on hydrate), Push grayed.
  await expectOwnerEditor(page);
  return shareId;
}
