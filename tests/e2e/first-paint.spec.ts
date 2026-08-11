import { test, expect, type Page } from "@playwright/test";
import { trackJsErrors } from "./helpers/errors";
import { createNotebook } from "./helpers/session";

/**
 * The shell's critical path.
 *
 * The shell used to load twelve classic scripts on every route. Measured on
 * production they held the parser for 223ms of a 378ms first contentful paint,
 * fetching 164KB of KaTeX, Prism, Monaco and sortable before the home page
 * (which uses none of them) drew anything. v0.19.5 deferred them; they are
 * loaded per route now, by `public/script-loader.js`.
 *
 * Three invariants pay for that, none of them visible in the markup:
 *
 * 1. Nothing blocks the parser. Scripts are inserted, not parsed.
 *
 * 2. KaTeX and Prism are UMD bundles that register as anonymous AMD modules
 *    and never assign their globals if `define` exists when they run. Tag
 *    order used to prevent that, which stops being true once loading depends
 *    on the route, so the loader masks `define` while they load. The failure
 *    is silent: math and syntax highlighting simply stop rendering.
 *
 * 3. Hydration waits for the route's scripts. `/pkg/` is served immutable
 *    while these are `no-cache`, so on a repeat visit the wasm is ready from
 *    cache while they are still being fetched, and wasm-bindgen resolves
 *    `js_namespace` globals at call time.
 */

/** Shell scripts are the classic ones; the pkg bundle loads via dynamic import. */
const SHELL_SCRIPTS = "script[src]:not([type=module])";

test.describe("Shell critical path", () => {
  /** Paths of the scripts a page actually fetched, excluding the pkg bundle. */
  async function fetchedScripts(page: Page): Promise<string[]> {
    return page.evaluate(() =>
      performance
        .getEntriesByType("resource")
        .map((r) => new URL(r.name).pathname)
        .filter((p) => p.endsWith(".js") && !p.includes("/pkg/")),
    );
  }

  test("no shell script blocks the parser", async ({ request }) => {
    // Asserted against the raw response, not the live DOM. A script the loader
    // inserts reports `defer === false` and `async === false` like a
    // parser-blocking tag does, because those properties describe the markup
    // attributes rather than how the element got there. Only the served HTML
    // distinguishes them.
    const html = await (await request.get("/")).text();
    const srcTags = [
      ...html.matchAll(/<script\b[^>]*\bsrc=["']([^"']+)["'][^>]*>/g),
    ];

    const blocking = srcTags
      .filter((m) => !m[0].includes("defer") && !m[0].includes("async"))
      .filter((m) => !m[1].includes("/pkg/"))
      .map((m) => m[1]);

    expect(
      blocking,
      `parser-blocking shell scripts: ${blocking.join(", ")}`,
    ).toEqual([]);
  });

  test("the home page does not fetch the markdown libraries", async ({
    page,
  }) => {
    // KaTeX and Prism are 117KB and the home page is a list of notebook cards.
    // Monaco, sortable and the executor are still loaded everywhere: Rust
    // reaches for their globals synchronously from mount effects, so they
    // cannot go lazy until those call sites can await.
    await page.goto("/");
    await page.waitForLoadState("load");

    const fetched = await fetchedScripts(page);
    // Guard the guard: zero scripts would pass the assertion below.
    expect(fetched.some((p) => p.includes("storage"))).toBe(true);

    const unwanted = fetched.filter(
      (p) => p.includes("katex") || p.includes("prism"),
    );
    expect(
      unwanted,
      `home fetched markdown libraries it does not use: ${unwanted.join(", ")}`,
    ).toEqual([]);
  });

  test("a notebook route still renders math and highlighted code", async ({
    page,
  }) => {
    // The silent failure mode: KaTeX and Prism are UMD bundles that register
    // as anonymous AMD modules if Monaco's loader defined `define` first, and
    // then simply never render. Route-dependent loading removes the ordering
    // that used to prevent it, so the loader masks `define` instead.
    await page.goto("/public/cannon");
    await expect(page.locator(".view-only-notebook")).toBeVisible({
      timeout: 15_000,
    });

    await expect
      .poll(() => page.locator(".katex").count(), { timeout: 15_000 })
      .toBeGreaterThan(0);
    await expect
      .poll(() => page.locator("code .token, pre .token").count(), {
        timeout: 15_000,
      })
      .toBeGreaterThan(0);
  });

  test("navigating from home tops up the markdown libraries", async ({
    page,
  }) => {
    // A client-side navigation never re-runs the shell, so the route's own
    // scripts have to be fetched by the router effect in `App`.
    await page.goto("/");
    await page.waitForLoadState("load");
    expect((await fetchedScripts(page)).some((p) => p.includes("katex"))).toBe(
      false,
    );

    await page.locator("a[href^='/public/']").first().click();

    await expect
      .poll(
        async () =>
          (await fetchedScripts(page)).some((p) => p.includes("katex")),
        { timeout: 20_000 },
      )
      .toBe(true);
  });

  test("UMD globals survive Monaco's AMD loader", async ({ page }) => {
    // Asserted on the editor route, the one place both the UMD bundles and
    // Monaco's AMD loader are present at once, which is the only situation
    // where they can collide.
    await createNotebook(page);

    const globals = await page.evaluate(() => {
      const w = window as never as Record<string, unknown>;
      return {
        katex: typeof w.IronpadKaTeX,
        prism: typeof w.IronpadPrism,
        monaco: typeof w.IronpadMonaco,
        storage: typeof w.IronpadStorage,
        executor: typeof w.IronpadExecutor,
      };
    });

    expect(globals).toEqual({
      katex: "object",
      prism: "object",
      monaco: "object",
      storage: "object",
      executor: "object",
    });
  });

  test("the app still hydrates behind the loaded shell", async ({ page }) => {
    // Hydration waits on the loader's promise. If it never settles the page
    // serves its SSR markup and stays dead, which no static check would
    // notice. Creating a notebook needs both halves of the pairing: a mounted
    // app, and `storage.js` having run to define IndexedDB access.
    const jsErrors = trackJsErrors(page);

    // `createNotebook` already waits for `.ironpad-editor`, so reaching this
    // line at all is the hydration assertion.
    await createNotebook(page);

    expect(jsErrors).toEqual([]);
  });

  test("first paint does not wait for the shell scripts", async ({ page }) => {
    // Delay every shell script and check that first paint does not move.
    //
    // The previous version compared FCP against the last script's responseEnd,
    // which is only meaningful while the scripts are still in flight at paint
    // time. On a fast run they finish first and the comparison inverts with
    // nothing wrong, which is exactly how it failed under full-suite load.
    // Injecting the delay makes the property deterministic instead of relying
    // on the machine being slow.
    const DELAY_MS = 1500;

    await page.route(/\.js(\?|$)/, async (route) => {
      if (route.request().url().includes("/pkg/")) return route.continue();
      await new Promise((r) => setTimeout(r, DELAY_MS));
      return route.continue();
    });

    await page.goto("/");
    const fcp = await page.evaluate(
      () =>
        new Promise<number | null>((resolve) => {
          const find = () =>
            performance
              .getEntriesByType("paint")
              .find((p) => p.name === "first-contentful-paint");
          if (find()) return resolve(find()!.startTime);
          const obs = new PerformanceObserver(() => {
            if (find()) {
              obs.disconnect();
              resolve(find()!.startTime);
            }
          });
          obs.observe({ type: "paint", buffered: true });
          setTimeout(() => resolve(find()?.startTime ?? null), 5000);
        }),
    );

    expect(fcp, "no paint was recorded").not.toBeNull();
    // Painting before the scripts could possibly have landed is the whole
    // claim. Blocking scripts would push this past the delay.
    expect(fcp!).toBeLessThan(DELAY_MS);
  });
});
