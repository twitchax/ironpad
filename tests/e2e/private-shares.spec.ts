import { test, expect, Page } from "@playwright/test";
import { loginTestUser } from "./helpers/auth";
import { shareMutable } from "./helpers/mutable";
import { createNotebook } from "./helpers/session";

/**
 * PRD-0061: private mutable shares + READ grants by GitHub handle.
 *
 * The owner flips a share private from the metadata panel's Access section;
 * anonymous readers and non-grantees get an explicit denial (with a sign-in
 * prompt), grantees read normally, and the anonymous surfaces (OG card,
 * oEmbed) 404 rather than leak the title.
 */

const BASE = "http://localhost:3111";

/** Expand the metadata section and its Access controls. */
async function openAccessControls(page: Page): Promise<void> {
  await page
    .locator(".view-only-shared-header", { hasText: "Notebook Metadata" })
    .click();
  await expect(page.locator(".ironpad-access-toggle")).toBeVisible({
    timeout: 15_000,
  });
}

test.describe("Private mutable shares (PRD-0061)", () => {
  test("private toggle denies strangers; a READ grant admits by handle", async ({
    page,
    browser,
    request,
  }) => {
    test.setTimeout(180_000);

    // The grant target must exist as a user first (grants link to user
    // records); one sign-in creates it.
    const granteeCtx = await browser.newContext();
    const granteePage = await granteeCtx.newPage();
    await loginTestUser(granteePage, "carol");

    // Owner publishes and flips private.
    await loginTestUser(page, "dave");
    await createNotebook(page);
    const shareId = await shareMutable(page);
    await expect(page.locator(".ironpad-push-button")).toBeVisible({
      timeout: 30_000,
    });
    await openAccessControls(page);
    await page.locator(".ironpad-access-toggle input").check();
    await expect(
      page.locator(".ironpad-toast-title", { hasText: "private" }),
    ).toBeVisible({ timeout: 15_000 });

    // Anonymous: explicit denial with the sign-in prompt, and no title leak
    // in the raw SSR body (what unfurlers see).
    const anon = await browser.newContext();
    try {
      const anonPage = await anon.newPage();
      await anonPage.goto(`/mutable/${shareId}`);
      await expect(
        anonPage.locator(".ironpad-error-boundary-message", {
          hasText: "This notebook is private.",
        }),
      ).toBeVisible({ timeout: 30_000 });
      await expect(
        anonPage.locator(".ironpad-error-boundary-hint a"),
      ).toHaveAttribute("href", "/auth/github");

      const raw = await (
        await anon.request.get(`${BASE}/mutable/${shareId}`)
      ).text();
      expect(raw).not.toContain("Untitled Notebook");

      // The anonymous surfaces refuse rather than leak.
      expect(
        (await anon.request.get(`${BASE}/og/mutable/${shareId}.png`)).status(),
      ).toBe(404);
      expect(
        (
          await anon.request.get(
            `${BASE}/oembed?url=${encodeURIComponent(
              `${BASE}/mutable/${shareId}`,
            )}`,
          )
        ).status(),
      ).toBe(404);
    } finally {
      await anon.close();
    }

    // A signed-in non-grantee is denied with the no-access copy.
    await granteePage.goto(`/mutable/${shareId}`);
    await expect(
      granteePage.locator(".ironpad-error-boundary-message", {
        hasText: "This notebook is private.",
      }),
    ).toBeVisible({ timeout: 30_000 });
    await expect(
      granteePage.locator(".ironpad-error-boundary-hint", {
        hasText: "does not have access",
      }),
    ).toBeVisible();

    // Grant carol by handle; she now reads the published notebook.
    await page.locator(".ironpad-access-add input").fill("carol");
    await page.locator(".ironpad-access-add button").click();
    await expect(
      page.locator(".ironpad-access-grant", { hasText: "@carol" }),
    ).toBeVisible({ timeout: 15_000 });

    await granteePage.goto(`/mutable/${shareId}`);
    await expect(granteePage.locator(".view-only-notebook")).toBeVisible({
      timeout: 30_000,
    });

    // Revoke: denied again.
    await page.locator(".ironpad-access-revoke").click();
    await expect(page.locator(".ironpad-access-grant")).toHaveCount(0);
    await granteePage.goto(`/mutable/${shareId}`);
    await expect(
      granteePage.locator(".ironpad-error-boundary-message", {
        hasText: "This notebook is private.",
      }),
    ).toBeVisible({ timeout: 30_000 });

    // Granting a handle that never signed in errors with the explanation.
    await page.locator(".ironpad-access-add input").fill("never-signed-in");
    await page.locator(".ironpad-access-add button").click();
    await expect(
      page.locator(".ironpad-toast-title", { hasText: "Could not grant" }),
    ).toBeVisible({ timeout: 15_000 });

    await granteeCtx.close();
  });
});
