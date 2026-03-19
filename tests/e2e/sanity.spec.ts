import { test, expect } from "@playwright/test";

// Placeholder: ensures Playwright is properly configured and can launch a browser.
// Real smoke tests are added by T-046+.
test("playwright is configured", async ({ page }) => {
  // Verify the server is reachable (webServer config starts it).
  const response = await page.goto("/");
  expect(response).not.toBeNull();
  expect(response!.status()).toBeLessThan(500);
});

// UAT-001: COOP/COEP headers enable crossOriginIsolated + SharedArrayBuffer.
test("crossOriginIsolated is enabled (COOP/COEP headers)", async ({
  page,
}) => {
  const response = await page.goto("/");

  // Verify the response headers are set.
  const headers = response!.headers();
  expect(headers["cross-origin-opener-policy"]).toBe("same-origin");
  expect(headers["cross-origin-embedder-policy"]).toBe("require-corp");

  // Verify the browser sees itself as cross-origin isolated.
  const isolated = await page.evaluate(() => window.crossOriginIsolated);
  expect(isolated).toBe(true);

  // SharedArrayBuffer must be available (required for rayon thread pool).
  const hasSAB = await page.evaluate(
    () => typeof SharedArrayBuffer !== "undefined"
  );
  expect(hasSAB).toBe(true);
});
