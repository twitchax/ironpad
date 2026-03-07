import { test, expect } from "@playwright/test";

// Placeholder: ensures Playwright is properly configured and can launch a browser.
// Real smoke tests are added by T-046+.
test("playwright is configured", async ({ page }) => {
  // Verify the server is reachable (webServer config starts it).
  const response = await page.goto("/");
  expect(response).not.toBeNull();
  expect(response!.status()).toBeLessThan(500);
});
