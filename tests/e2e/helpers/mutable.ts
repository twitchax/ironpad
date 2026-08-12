import { APIRequestContext, expect, Page } from "@playwright/test";

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

/**
 * Save to Account (PRD-0064): uploads the notebook into the signed-in user's
 * account, deletes the local copy, and hard-navigates to /mutable/{id} where
 * the owner's editor mounts. Returns the minted id.
 *
 * Share Mutable minus the publish — the same move-never-copy flow, so it
 * lives beside it rather than in a second hand-rolled spelling.
 */
export async function saveToAccount(
  page: Page,
  opts: { onConfirm?: (message: string) => void } = {},
): Promise<string> {
  // Save to Account is confirm-gated: it deletes the browser-local copy and
  // the local version history with it, so the consequences are named at the
  // moment of the decision. Playwright AUTO-DISMISSES an unhandled dialog,
  // which would silently turn every call here into a no-op that then times
  // out waiting for a navigation nobody asked for.
  page.once("dialog", (d) => {
    opts.onConfirm?.(d.message());
    void d.accept();
  });
  await menuClick(page, "Save to Account");
  await expect(page).toHaveURL(/\/mutable\/[a-f0-9]{16}/, { timeout: 30_000 });
  const shareId = page.url().match(/\/mutable\/([a-f0-9]{16})/)![1];
  // The toast rides sessionStorage across the hard navigation.
  await expect(
    page.locator(".ironpad-toast-title", { hasText: "Saved to Your Account" }),
  ).toBeVisible({ timeout: 15_000 });
  // The leftover-copy toast replaces the success one when the local delete
  // did not take (PRD-0064 names it rather than swallowing it). It is the
  // failure surface, so it must never appear on a healthy save.
  await expect(
    page.locator(".ironpad-toast-title", {
      hasText: "Saved, With a Leftover Copy",
    }),
  ).toHaveCount(0);
  // The owner lands in the editor (auto-swap on hydrate). SSR still answers
  // 404 for an unpublished notebook, including to its owner, but it renders
  // the NEUTRAL placeholder rather than the reader's not-found panel while
  // the ownership probe is outstanding; the spec asserts that copy directly.
  await expectOwnerEditor(page);
  return shareId;
}

/**
 * Assert /og/mutable/{id}.png is a real card: PNG magic bytes plus the IHDR
 * width/height, because that is what an unfurler reads to size the preview.
 *
 * Also the warm-up half of a 404-after-withdrawal assertion — the card is
 * cached server-side, so a later 404 only means something if a 200 was
 * rendered and stored first.
 */
export async function expectLiveCard(
  request: APIRequestContext,
  shareId: string,
): Promise<void> {
  const res = await request.get(
    `http://localhost:3111/og/mutable/${shareId}.png`,
  );
  expect(res.status()).toBe(200);
  expect(res.headers()["content-type"]).toBe("image/png");
  const body = await res.body();
  expect(body.subarray(0, 8)).toEqual(
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
  );
  expect(body.readUInt32BE(16)).toBe(1200);
  expect(body.readUInt32BE(20)).toBe(630);
}
