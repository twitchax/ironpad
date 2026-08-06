import { Page } from "@playwright/test";

/**
 * The notebook (hamburger) menu contract — the ONE home for the toggle
 * selector and the open-and-click flow, shared by every spec that drives
 * the menu. A dropdown-class rename lands here once instead of breaking a
 * handful of specs individually while divergent hand-rolled spellings stay
 * green against markup they no longer match.
 */

export const MENU = '.ironpad-toolbar-dropdown-toggle[title="Notebook menu"]';

/** Open the notebook (hamburger) menu and click an item by its label. */
export async function menuClick(page: Page, label: string): Promise<void> {
  await page.locator(MENU).click();
  await page
    .locator(".ironpad-toolbar-dropdown-item", { hasText: label })
    .click();
}
