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
      // The suite's compiles include deliberate always-misses (failed
      // compiles are never cached; Force Recompile bypasses every layer),
      // all sharing one "local" bucket — production limits would trip on
      // suite scale, not on a real fault (this happened: notebook.spec and
      // shared-cells.spec failed only in the full run). The limiter itself
      // is covered by unit tests in compiler/admission.rs.
      IRONPAD_BUILD_RATE_BURST: "100000",
      IRONPAD_BUILD_RATE_PER_MIN: "100000",
      // Registers /auth/test-login (PRD-0053) so specs can mint real
      // sessions without GitHub. Production never sets this.
      IRONPAD_TEST_AUTH: "1",
      // Dummy OAuth credentials so the sign-in surface renders (the footer
      // hides it entirely when unconfigured). Nothing ever completes the
      // GitHub dance in e2e — test-login mints the sessions.
      GITHUB_CLIENT_ID: "test-client-id",
      GITHUB_CLIENT_SECRET: "test-client-secret",
    },
    url: "http://localhost:3111",
    reuseExistingServer: !process.env.CI,
    timeout: 600_000, // 10 min — a cold release build (surrealdb tree) exceeds 5
  },
});
