import { defineConfig, devices } from "@playwright/test";

/**
 * Playwright configuration for ironpad end-to-end tests.
 *
 * Uses cargo-leptos to build and serve the app before running tests.
 * Only Chromium is enabled for CI speed.
 */
export default defineConfig({
  testDir: "./tests/e2e",
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 2 : 0,
  workers: process.env.CI ? 1 : undefined,
  reporter: "html",

  use: {
    baseURL: "http://localhost:3111",
    trace: "on-first-retry",
  },

  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
  ],

  webServer: {
    command: "cargo leptos serve --release",
    env: {
      // Local target dirs lack cargo-check artifacts (the deploy image seeds
      // them), so the first live check would blow the 10s production budget
      // and silently skip. Tests get a patient budget instead.
      IRONPAD_LIVE_CHECK_TIMEOUT_SECS: "300",
    },
    url: "http://localhost:3111",
    reuseExistingServer: !process.env.CI,
    timeout: 300_000, // 5 min — cargo build can be slow
  },
});
