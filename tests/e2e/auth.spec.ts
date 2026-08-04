import { test, expect } from "@playwright/test";
import { createNotebook } from "./helpers/session";
import { loginTestUser, logout } from "./helpers/auth";

/**
 * PRD-0053: GitHub-account sign-in. The e2e server runs with
 * IRONPAD_TEST_AUTH=1 (real sessions without the GitHub dance) and dummy
 * OAuth credentials (so the sign-in surface renders). The route-absence
 * guarantee — /auth/test-login does not exist unless the env is set — is
 * covered by unit tests in ironpad-server/src/auth.rs, which spawn routers
 * with the gate off.
 */

const BASE = "http://localhost:3111";

test.describe("Accounts (PRD-0053)", () => {
  test("test login signs in, the footer shows the identity, and logout clears it", async ({
    page,
  }) => {
    test.setTimeout(60_000);

    await loginTestUser(page, "carol");

    // The auth surface lives in the header, so it must be visible on the
    // home page itself — this shipped as a footer widget in 0.15.0 and was
    // invisible exactly where a visitor looks for login.
    await expect(page.locator(".ironpad-auth-login")).toHaveText("@carol", {
      timeout: 15_000,
    });

    // Sign out from the header link: session gone, sign-in link back.
    await page.locator(".ironpad-auth-action", { hasText: "Sign out" }).click();
    await expect(page.locator(".ironpad-home")).toBeVisible({
      timeout: 15_000,
    });
    await expect(
      page.locator(".ironpad-auth-action", {
        hasText: "Sign in with GitHub",
      }),
    ).toBeVisible({ timeout: 15_000 });
  });

  test("the session cookie carries the full flag set", async ({ request }) => {
    const res = await request.get(`${BASE}/auth/test-login?login=flagcheck`, {
      maxRedirects: 0,
    });
    expect(res.status()).toBe(303);
    const cookies = res.headersArray().filter((h) => h.name.toLowerCase() === "set-cookie");
    const session = cookies.find((h) => h.value.startsWith("ironpad_session="));
    expect(session, "a session cookie must be set").toBeTruthy();
    for (const flag of ["HttpOnly", "Secure", "SameSite=Lax", "Path=/"]) {
      expect(session!.value).toContain(flag);
    }
  });

  test("the sign-in link starts the OAuth dance with a CSRF nonce cookie", async ({
    request,
  }) => {
    const res = await request.get(
      `${BASE}/auth/github?redirect_to=/local/abc`,
      { maxRedirects: 0 },
    );
    expect(res.status()).toBe(303);
    expect(res.headers()["location"]).toContain(
      "https://github.com/login/oauth/authorize",
    );
    const cookies = res.headersArray().filter((h) => h.name.toLowerCase() === "set-cookie");
    const nonce = cookies.find((h) => h.value.startsWith("ironpad_oauth_state="));
    expect(nonce, "the CSRF nonce cookie must be set").toBeTruthy();
    expect(nonce!.value).toContain("Path=/auth");
  });

  test("publishing a mutable share requires a session", async ({ page }) => {
    test.setTimeout(60_000);

    // Anonymous: the Share Mutable action exists but the server refuses it
    // with a toast that says what to do.
    await createNotebook(page);
    await page
      .locator('.ironpad-toolbar-dropdown-toggle[title="Notebook menu"]')
      .click();
    await page
      .locator(".ironpad-toolbar-dropdown-item", { hasText: "Share Mutable" })
      .click();
    await expect(
      page.locator(".ironpad-toast-body", { hasText: "sign in" }),
    ).toBeVisible({ timeout: 30_000 });
  });

  test("logged-out users keep full anonymous functionality", async ({
    page,
  }) => {
    test.setTimeout(60_000);
    await logout(page);
    // Creating, editing, and listing notebooks never touches auth.
    await createNotebook(page);
    await page.locator(".ironpad-add-cell-btn").first().click();
    await expect(page.locator(".ironpad-cell-card")).toHaveCount(1);
  });
});
