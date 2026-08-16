import { test, expect } from "@playwright/test";
import { trackJsErrors } from "./helpers/errors";
import { POD_HOST, recordPodRequests, blockPodCdn } from "./helpers/browserpod";
import { createNotebook, ADD_LINUX } from "./helpers/session";
import { menuClick } from "./helpers/menu";

/**
 * uat-007 (a CDN outage degrades to an inline notice) and uat-008 (embeds
 * refuse Linux cells). Both were asserted by PRD-0066 and neither had a test;
 * review found both FALSE, for unrelated reasons.
 *
 * Neither test boots a pod, so both belong in the default gate. That is not a
 * compromise — a boot costs 10 tokens of a ~1,000-boot month, and the failure
 * modes under test are exactly the ones where no pod is reached.
 *
 * **Why uat-008 asserts the SERVER's HTML.** The refusal used to be computed
 * from `window.crossOriginIsolated` at render time. Hydration reads that, but
 * tachys does not re-write attributes when hydrating server HTML
 * (`html/attribute/value.rs`: `if !FROM_SERVER { set_attribute(...) }`), so
 * the SSR markup won and a cross-origin embed shipped a Run button with no
 * `disabled`, labelled runnable, that silently did nothing.
 *
 * It passed everything we had because **a same-origin Playwright page IS
 * cross-origin isolated** — SSR and hydrate agreed, so the bug could not fire
 * where anyone looked. A DOM assertion here would have the same blind spot,
 * which is why the load-bearing check reads the raw response body.
 */

const BASE = "http://localhost:3111";
const HASH = /\/shared\/([0-9a-f]{16})/;

/** Author a Linux cell through the UI and share it, returning the hash. */
async function shareLinuxNotebook(page: import("@playwright/test").Page) {
  await createNotebook(page);
  await page.locator(ADD_LINUX).first().click();
  await expect(page.locator(".ironpad-cell-type-badge--linux")).toHaveCount(1);

  await menuClick(page, "Share Immutable");
  const toastBody = page.locator(".ironpad-toast-body", { hasText: "/shared/" });
  await expect(toastBody).toContainText("/shared/", { timeout: 30_000 });
  const match = (await toastBody.textContent())!.match(HASH);
  expect(match, "the share toast must carry a hash").not.toBeNull();
  return match![1];
}

test.describe("uat-008: embeds refuse Linux cells", () => {
  test("the refusal is in the server's HTML, not applied later by script", async ({
    page,
    request,
  }) => {
    test.setTimeout(300_000);
    const hash = await shareLinuxNotebook(page);

    // The assertion that actually catches an SSR/hydrate divergence. A
    // rendered-DOM check passes on a same-origin page whether or not
    // hydration fixed it up, which is how the original bug survived.
    const res = await request.get(`${BASE}/embed/shared/${hash}`);
    expect(res.status()).toBe(200);
    const html = await res.text();

    expect(html, "the run control must ship disabled from the server").toMatch(
      /<button[^>]*\bdisabled\b/,
    );
    expect(html, "the embed must say why it refuses").toContain("cross-origin-isolated page");
    expect(html, "an embed must not name the pod CDN").not.toContain(POD_HOST);
  });

  test("an embedded Linux cell boots nothing when a reader clicks it", async ({ page }) => {
    test.setTimeout(300_000);
    const hash = await shareLinuxNotebook(page);

    const errors = trackJsErrors(page);
    const pod = recordPodRequests(page);
    await page.goto(`${BASE}/embed/shared/${hash}`);
    await expect(page.locator(".view-only-cell--linux")).toBeVisible({ timeout: 30_000 });
    await page.waitForTimeout(3_000); // hydration (suite convention)

    const run = page.locator(".view-only-cell--linux .view-only-run-button");
    await expect(run).toBeDisabled();
    await expect(page.locator(".view-only-inert-notice")).toContainText("embed");

    // A disabled button swallows the click; assert the outcome anyway, since
    // the bug under test was precisely a control that looked live.
    await run.click({ force: true }).catch(() => {});
    await page.waitForTimeout(1_000);
    expect(pod, "an embed must never reach the pod CDN").toEqual([]);
    expect(errors).toHaveLength(0);
  });
});

test.describe("uat-007: a CDN outage degrades to an inline notice", () => {
  test("a failed boot explains itself and leaves the cell usable", async ({ page }) => {
    test.setTimeout(600_000);
    const errors = trackJsErrors(page);
    await blockPodCdn(page);

    await createNotebook(page);
    await page.locator(ADD_LINUX).first().click();
    await expect(page.locator(".ironpad-cell-type-badge--linux")).toHaveCount(1);

    // The editor offers no Run button for a Linux cell; Preview is where it
    // runs, which is what the cell's own notice tells the author.
    // Scoped by aria-label, not by role+name: the metadata panel carries
    // "Preview image" labels that resolve to the same accessible name and make
    // a bare getByRole ambiguous under strict mode.
    await page.locator('button[aria-label="Preview"]').click();
    const run = page.locator(".view-only-cell--linux .view-only-run-button");
    await expect(run).toBeEnabled({ timeout: 30_000 });
    await run.click();

    // The compile is real and slow; the failure under test happens after it,
    // when the runtime reaches for the CDN. A boot that never produced output
    // renders through `.view-only-error` rather than the terminal's status
    // line — `show_terminal` deliberately withholds an empty terminal so a
    // real diagnosis is not paired with a panel claiming there was no output.
    // Asserting on the terminal status was my first guess and it was wrong.
    const failure = page.locator(".view-only-cell--linux .view-only-error");
    await expect(failure).toBeVisible({ timeout: 420_000 });
    await expect(failure).toContainText(/unreachable|could not|failed|error/i, {
      timeout: 420_000,
    });

    // uat-007's real claim: an ERROR, not a hang. The cell must leave its busy
    // state so the reader can retry, and the page must not be poisoned.
    await expect(run).toBeEnabled({ timeout: 60_000 });
    expect(errors).toHaveLength(0);
  });
});
