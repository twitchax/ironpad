import { defineConfig, devices } from "@playwright/test";
import { POD_HOST } from "./tests/e2e/helpers/browserpod";

/**
 * Playwright configuration for ironpad end-to-end tests.
 *
 * Uses cargo-leptos to build and serve the app before running tests.
 * Only Chromium is enabled for CI speed.
 *
 * ── The BrowserPod split (PRD-0066 T-014) ─────────────────────────────────
 *
 * A BrowserPod pod boot costs 10 tokens of a ~1,000-boot monthly allowance,
 * flat and duration-independent. A pod-dependent spec run is ~40 tokens, and
 * ten gate runs a day exhausts the month in under four weeks with no users
 * involved: the test suite, not visitors, is the dominant consumer.
 *
 * So the default `chromium` project cannot boot one, enforced twice over:
 *
 *  1. It ignores `tests/e2e/linux-pod/`, where every pod-booting spec lives.
 *  2. It launches Chromium with the CDN mapped to `~NOTFOUND`, so the host
 *     does not resolve at all. Convention alone would be one forgetful commit
 *     away from a spend; this is the browser refusing, which no spec placed in
 *     the wrong directory can talk its way past. `linux-cells.spec.ts` asserts
 *     the block is live, because a guard nobody checks is a guard that quietly
 *     stops working.
 *
 * The opt-in `linux-pod` project EXISTS only when IRONPAD_LINUX_POD_TESTS is
 * set — a project Playwright has never heard of cannot be run by a stray
 * `npx playwright test`. `cargo make test-linux-cells` sets it; `cargo make
 * uat` and CI never do.
 */
const POD_TESTS = !!process.env.IRONPAD_LINUX_POD_TESTS;

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
      testIgnore: "**/linux-pod/**",
      use: {
        ...devices["Desktop Chrome"],
        launchOptions: {
          // Chromium resolves this host to nothing, so no spec in the default
          // gate can spend a metered pod boot even by accident. Scoped to the
          // one host: everything else, including localhost, resolves normally.
          args: [`--host-resolver-rules=MAP ${POD_HOST} ~NOTFOUND`],
        },
      },
    },
    // Opt-in only. Absent unless IRONPAD_LINUX_POD_TESTS is set, so these
    // specs are unreachable from `npx playwright test` and from `cargo make
    // uat`.
    ...(POD_TESTS
      ? [
          {
            name: "linux-pod",
            testMatch: "**/linux-pod/**/*.spec.ts",
            use: { ...devices["Desktop Chrome"] },
          },
        ]
      : []),
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
      // Names the admin for PRD-0063, so the panel is covered by the same
      // suite as everything else rather than needing its own server. Safe to
      // pair with test auth: an instance that can mint a session for any user
      // is already fully compromised, so admin is not the escalation that
      // matters. Specs become the admin by signing in as this login, and any
      // other login exercises the denial path.
      IRONPAD_ADMIN_LOGIN: "ironpad-admin",
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
