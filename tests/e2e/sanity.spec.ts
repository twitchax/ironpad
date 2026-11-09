import { test, expect } from "@playwright/test";
import { trackJsErrors } from "./helpers/errors";

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

// trackJsErrors must see a panic in the app's own wasm. console_error_panic_hook
// reports it as a console.error (the follow-on "unreachable" trap is filtered as
// a cell panic), so a pageerror listener alone passed specs on a dead app. Cell
// panics reach the same console (Playwright forwards worker console messages to
// the page) and must stay out: they are surfaced inline as run errors.
test("trackJsErrors records app panics but not cell panics", async ({
  page,
}) => {
  const errors = trackJsErrors(page);
  await page.goto("/");

  const appPanic = "panicked at crates/ironpad-app/src/x.rs:1:1:\nboom";
  const cellPanic = "panicked at src/lib.rs:8:5:\nboom";
  const disposedPanic =
    "panicked at /root/.cargo/registry/src/index.crates.io-0000/reactive_graph-0.2.0/src/traits.rs:1:1:\n" +
    "Tried to access a reactive value that has already been disposed.";

  await page.evaluate((m) => console.error(m), appPanic);
  await expect.poll(() => errors.length).toBe(1);

  // Console messages arrive in order, so once the trailing app panic lands the
  // cell panic before it has been seen and dropped.
  await page.evaluate(
    ([cell, app]) => {
      console.error(cell);
      console.error(app);
    },
    [cellPanic, disposedPanic] as const,
  );
  await expect.poll(() => errors.length).toBeGreaterThanOrEqual(2);
  expect(errors).toEqual([appPanic, disposedPanic]);
});
