import { expect, Page } from "@playwright/test";

import { menuClick } from "./menu";

/**
 * The Share Mutable publish flow, shared by every spec that drives it. One
 * home: a change to the flow lands here once, instead of leaving a weaker
 * copy green against a broken flow. The generic menu contract lives in
 * `./menu` (re-exported here for existing importers).
 */

export { MENU, menuClick } from "./menu";

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
