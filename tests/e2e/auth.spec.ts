import { test, expect } from "@playwright/test";
import { loginTestUser, logout } from "./helpers/auth";
import { menuClick } from "./helpers/menu";
import { setCellSource } from "./helpers/monaco";
import { ADD_MARKDOWN, createNotebook } from "./helpers/session";

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
    //
    // Signed in is the same portrait-over-label stack as signed out, with
    // "Sign out" UNDER the avatar; the handle rides the tooltip, since a
    // third row does not fit a 48px header.
    const auth = page.locator(".ironpad-auth");
    await expect(auth).toHaveAttribute("title", "@carol", { timeout: 15_000 });
    await expect(auth.locator(".ironpad-auth-action")).toHaveText("Sign out");
    // Stacked, not inline: the action's box starts below the portrait's. The
    // test user has no GitHub picture, which is exactly the case that must
    // still render a portrait — the silhouette — rather than leaving "Sign
    // out" floating alone in the header.
    const stacked = await auth.evaluate((el) => {
      const portrait = el.querySelector(
        ".ironpad-auth-avatar, .ironpad-auth-signin-avatar",
      );
      const action = el.querySelector(".ironpad-auth-action");
      if (!portrait || !action) return null;
      return (
        action.getBoundingClientRect().top >=
        portrait.getBoundingClientRect().bottom
      );
    });
    expect(stacked, "no portrait rendered above Sign out").toBe(true);

    // Sign out from the header link: session gone, and the compact
    // empty-portrait sign-in control is back.
    await page.locator(".ironpad-auth-action", { hasText: "Sign out" }).click();
    await expect(page.locator(".ironpad-home")).toBeVisible({
      timeout: 15_000,
    });
    await expect(page.locator(".ironpad-auth-signin")).toBeVisible({
      timeout: 15_000,
    });
    await expect(page.locator(".ironpad-auth-signin-label")).toHaveText(
      "Sign in",
    );
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
    // Accept the move confirm; an auto-dismissed one never reaches the server.
    page.once("dialog", (d) => void d.accept());
    await menuClick(page, "Share Mutable");
    await expect(
      page.locator(".ironpad-toast-body", { hasText: "sign in" }),
    ).toBeVisible({ timeout: 30_000 });
  });

  // Logout is a GET and the markdown sanitizer keeps relative image URLs, so
  // `![](/auth/logout)` in a notebook signed out everyone who viewed it: the
  // image request is same-origin, so SameSite does not hold the cookie back.
  test("a notebook embedding /auth/logout does not sign its reader out", async ({
    page,
    browser,
  }) => {
    test.setTimeout(120_000);

    // Author the trap anonymously and share it.
    await createNotebook(page);
    await page.locator(ADD_MARKDOWN).first().click();
    const cell = page.locator(".ironpad-cell-card").first();
    await cell.locator(".ironpad-markdown-cell-preview").dblclick();
    await expect(cell.locator(".monaco-editor").first()).toBeVisible({
      timeout: 15_000,
    });
    await setCellSource(page, cell, "![](/auth/logout)");
    await cell.locator(".ironpad-cell-header").click();
    // setValue never focused Monaco, so no focusout commits the edit; Ctrl+S
    // does (as in routes.spec.ts), and the preview then shows the trap.
    await page.keyboard.press("Control+s");
    await expect(page.locator(".ironpad-status-bar")).toContainText("Saved:", {
      timeout: 10_000,
    });
    await expect(cell.locator('img[src$="/auth/logout"]')).toHaveCount(1);
    await menuClick(page, "Share Immutable");
    const toastBody = page.locator(".ironpad-toast-body", {
      hasText: "/shared/",
    });
    await expect(toastBody).toContainText("/shared/", { timeout: 30_000 });
    const hash = (await toastBody.textContent())!.match(
      /\/shared\/([0-9a-f]{16})/,
    )![1];

    // A signed-in reader in a fresh context, so nothing the author's page
    // already fetched can stand in for the reader's own request.
    const reader = await browser.newContext({ baseURL: BASE });
    try {
      const readerPage = await reader.newPage();
      await loginTestUser(readerPage, "dave");
      const logoutStatuses: number[] = [];
      readerPage.on("response", (r) => {
        if (new URL(r.url()).pathname === "/auth/logout") {
          logoutStatuses.push(r.status());
        }
      });

      await readerPage.goto(`/shared/${hash}`);
      const trap = readerPage.locator('img[src$="/auth/logout"]');
      await expect(trap).toHaveCount(1, { timeout: 30_000 });
      // The browser has made the request once the image settles (loaded or
      // broken), so the reload below cannot outrun it.
      await expect
        .poll(() => trap.evaluate((img: HTMLImageElement) => img.complete))
        .toBe(true);

      await readerPage.reload();
      await expect(readerPage.locator(".ironpad-auth")).toHaveAttribute(
        "title",
        "@dave",
        { timeout: 15_000 },
      );
      // ...and the reason is that the request was refused, not skipped.
      expect(logoutStatuses.length).toBeGreaterThan(0);
      expect(logoutStatuses.every((status) => status === 403)).toBe(true);
    } finally {
      await reader.close();
    }
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
