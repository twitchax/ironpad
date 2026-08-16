/**
 * BrowserPod CDN helpers (PRD-0066).
 *
 * Every pod boot spends 10 tokens of a ~1,000-boot monthly allowance, flat and
 * duration-independent, so a spec that boots one costs real money and the test
 * suite outspends visitors by an order of magnitude if left unchecked. Those
 * specs live in `tests/e2e/linux-pod/` and run only under
 * `cargo make test-linux-cells`; everything in the default gate must be able
 * to make its point without a pod.
 *
 * This module is the shared vocabulary for doing that: the hostname, a request
 * recorder for asserting silence, and a router for simulating the CDN being
 * down. Booting pulls FIVE assets from that host (browserpod.js, kernel.wasm,
 * cache.wasm, worker.js, opfs_worker.js), so both helpers key on the HOST and
 * never on one script name.
 */
import { Page, Route } from "@playwright/test";

/** The BrowserPod runtime CDN. Contacting it at all means a boot is starting. */
export const POD_HOST = "rt.browserpod.io";

/**
 * Record every request the page issues to the BrowserPod CDN.
 *
 * Listens for `request`, which fires when the browser ISSUES a request —
 * before DNS, connection or response. That is deliberate: the default
 * Playwright project launches Chromium with the host mapped to `~NOTFOUND`
 * (see `playwright.config.ts`), so a request that got as far as the network
 * would fail regardless, and asserting on responses would pass whether or not
 * the app tried. The attempt is the thing being forbidden.
 */
export function recordPodRequests(page: Page): string[] {
  const urls: string[] = [];
  page.on("request", (req) => {
    if (new URL(req.url()).hostname === POD_HOST) urls.push(req.url());
  });
  return urls;
}

/**
 * Fail every request to the BrowserPod CDN, simulating an outage.
 *
 * The cheap, deterministic way to test uat-007: a boot that never reaches the
 * SDK spends nothing, so "the CDN is down" costs zero tokens where booting a
 * pod and killing it would cost ten. Aborts with `connectionfailed` because
 * that is what an unreachable host looks like to a dynamic `import()`.
 */
export async function blockPodCdn(page: Page): Promise<void> {
  await page.route(
    (url) => url.hostname === POD_HOST,
    (route: Route) => route.abort("connectionfailed"),
  );
}
