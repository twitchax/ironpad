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
    // never allowed to clear.
    const rows = page.locator(".ironpad-admin-table tbody tr");
    await expect(rows).toHaveCount(4);
    await expect(page.locator(".ironpad-admin-table")).toContainText("blobs");
    await expect(page.locator(".ironpad-admin-table")).toContainText("never");
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
