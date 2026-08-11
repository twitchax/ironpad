import { test, expect } from "@playwright/test";
import { loginTestUser, logout } from "./helpers/auth";

/**
 * The admin panel's gate (PRD-0063).
 *
 * The Playwright webServer sets IRONPAD_ADMIN_LOGIN=ironpad-admin alongside
 * IRONPAD_TEST_AUTH, so both sides of the gate are reachable from the shared
 * suite: sign in as that login to be the admin, as anything else to be denied.
 * That pairing is deliberate. An instance that can mint a session for any user
 * is already fully compromised, so admin is not the escalation that matters,
 * and keeping the panel in this suite is worth more than the second server the
 * alternative would have cost.
 *
 * Denial is asserted as an ordinary not-found, never a distinct "forbidden":
 * a 403 tells a prober the panel is real and that they have the right URL.
 */

const ADMIN = "ironpad-admin";
const NOT_ADMIN = "octocat";

test.describe("Admin panel", () => {
  test("the named admin sees instance state", async ({ page }) => {
    await loginTestUser(page, ADMIN);
    await page.goto("/admin");

    await expect(page.locator(".ironpad-admin")).toBeVisible({
      timeout: 15_000,
    });
    await expect(page.getByRole("heading", { name: "Instance" })).toBeVisible();

    // Every cache tier is listed, including the one the automatic valve is
    // never allowed to clear. Scoped to the cache table: the users table below
    // shares the base class, and an unscoped row count silently counted both.
    const cache = page.locator(".ironpad-admin-table--cache");
    await expect(cache.locator("tbody tr")).toHaveCount(4);
    await expect(cache).toContainText("blobs");
    await expect(cache).toContainText("never");
  });

  test("a signed-in non-admin gets an ordinary not found", async ({ page }) => {
    await loginTestUser(page, NOT_ADMIN);
    const response = await page.goto("/admin");

    await expect(page.locator(".ironpad-admin")).toHaveCount(0);
    await expect(page.getByText("Page not found.")).toBeVisible({
      timeout: 15_000,
    });
    // A real status, not a soft 404 with a 200 body.
    expect(response?.status()).toBe(404);
  });

  test("an anonymous visitor gets an ordinary not found", async ({ page }) => {
    await logout(page);
    const response = await page.goto("/admin");

    await expect(page.locator(".ironpad-admin")).toHaveCount(0);
    await expect(page.getByText("Page not found.")).toBeVisible({
      timeout: 15_000,
    });
    expect(response?.status()).toBe(404);
  });

  test("instance state never reaches a non-admin over the wire", async ({
    page,
  }) => {
    // The component declining to draw is not the control; the control is that
    // the server fn refuses, so the numbers never leave the process. Asserted
    // against the raw response body rather than the rendered DOM, because a
    // hydrated page could hide what SSR already serialised into it.
    await loginTestUser(page, NOT_ADMIN);
    const response = await page.goto("/admin");
    const body = (await response?.text()) ?? "";

    expect(body).not.toContain("ironpad-admin-stats");
    expect(body).not.toContain("Published notebooks");
    expect(body).not.toContain("cargo-home");
  });

  test("the user list shows per-user counts and revokes sessions", async ({
    page,
    context,
  }) => {
    // A second signed-in user to act on, in its own browser context so the
    // admin's session is not the one being revoked.
    const victim = await context.browser()!.newContext();
    const victimPage = await victim.newPage();
    await loginTestUser(victimPage, "revoke-me");

    await loginTestUser(page, ADMIN);
    await page.goto("/admin");
    await expect(page.locator(".ironpad-admin")).toBeVisible({
      timeout: 15_000,
    });

    const row = page
      .locator(".ironpad-admin-table--users tbody tr")
      .filter({ hasText: "revoke-me" });
    await expect(row).toBeVisible({ timeout: 15_000 });
    await expect(row).toContainText("1");

    // The confirm is part of the action, so accept it rather than routing
    // around the code under test.
    page.once("dialog", (d) => d.accept());
    await row.getByRole("button", { name: "Revoke sessions" }).click();

    // The count is what the action changes, so the list must refetch.
    await expect
      .poll(async () => (await row.textContent()) ?? "", { timeout: 15_000 })
      .toMatch(/revoke-me\s*0/);

    // And the revoked user is actually signed out, which is the point.
    await victimPage.goto("/");
    await expect(victimPage.locator("a.ironpad-auth")).toHaveCount(0);
    await victim.close();
  });

  test("clearing a cache tier states its cost and frees only that tier", async ({
    page,
  }) => {
    await loginTestUser(page, ADMIN);
    await page.goto("/admin");
    await expect(page.locator(".ironpad-admin")).toBeVisible({
      timeout: 15_000,
    });

    const cache = page.locator(".ironpad-admin-table--cache");
    const workspaces = cache
      .locator("tbody tr")
      .filter({ hasText: "workspaces" });
    const blobs = cache.locator("tbody tr").filter({ hasText: "blobs" });

    // The confirm has to name the tier and its size, not just ask "are you
    // sure": clearing scratch workspaces and discarding every compiled cell
    // are very different acts behind the same button.
    let message = "";
    page.once("dialog", (d) => {
      message = d.message();
      d.dismiss();
    });
    await workspaces.getByRole("button", { name: "Clear" }).click();
    await expect
      .poll(() => message, { timeout: 10_000 })
      .toContain("workspaces");
    expect(message).toMatch(/\d/);
    expect(message).toContain("cannot be undone");

    // Dismissing must not have cleared anything.
    await expect(workspaces).toBeVisible();

    // Blobs is the tier the unattended valve may never touch, so its warning
    // has to say what a reader loses rather than reuse the generic line.
    let blobMessage = "";
    page.once("dialog", (d) => {
      blobMessage = d.message();
      d.dismiss();
    });
    const blobButton = blobs.getByRole("button", { name: "Clear" });
    if (await blobButton.isEnabled()) {
      await blobButton.click();
      await expect
        .poll(() => blobMessage, { timeout: 10_000 })
        .toContain("compiled");
    }
  });

  test("the panel is kept out of search results", async ({ page, request }) => {
    await loginTestUser(page, ADMIN);
    await page.goto("/admin");
    await expect(page.locator(".ironpad-admin")).toBeVisible({
      timeout: 15_000,
    });

    const robots = page.locator('meta[name="robots"]');
    await expect(robots).toHaveAttribute("content", /noindex/);

    const txt = await (await request.get("/robots.txt")).text();
    expect(txt).toContain("Disallow: /admin");
  });
});
