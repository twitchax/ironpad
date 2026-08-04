import { Page, expect } from "@playwright/test";

/**
 * Sign in via the env-gated test endpoint (PRD-0053). The Playwright
 * webServer sets IRONPAD_TEST_AUTH=1, which registers /auth/test-login; it
 * mints a real user + session through the same code paths as the GitHub
 * OAuth callback, so everything downstream (cookies, RBAC grants, footer
 * state) is the production surface.
 *
 * The synthetic account's GitHub id is `test-{login}`, so distinct logins
 * are distinct owners — use that to test cross-account authorization.
 */
export async function loginTestUser(page: Page, login: string): Promise<void> {
  await page.goto(`/auth/test-login?login=${login}`);
  // The endpoint 303s home once the session cookie is set.
  await expect(page.locator(".ironpad-home")).toBeVisible({ timeout: 15_000 });
}

/** Sign out via the logout route (clears the session cookie, lands home). */
export async function logout(page: Page): Promise<void> {
  await page.goto("/auth/logout");
  await expect(page.locator(".ironpad-home")).toBeVisible({ timeout: 15_000 });
}
