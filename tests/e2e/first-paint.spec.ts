import { test, expect } from "@playwright/test";
import { trackJsErrors } from "./helpers/errors";
import { createNotebook } from "./helpers/session";

/**
 * The shell's critical path.
 *
 * Every classic script in the shell is `defer`red. Measured against production
 * before that change, the twelve blocking scripts held the parser for 223ms of
 * a 378ms first contentful paint, fetching 164KB of KaTeX, Prism, Monaco and
 * sortable before the home page (which uses none of them) drew anything.
 *
 * Two invariants pay for that and neither is visible in the markup:
 *
 * 1. `defer`, never `async`. Deferred scripts still execute in document order,
 *    and KaTeX and Prism ship UMD bundles that must run BEFORE Monaco's AMD
 *    loader defines `define`, or they register as anonymous modules and their
 *    globals are never assigned. `async` would drop that ordering and the
 *    failure is silent: math and syntax highlighting simply stop working.
 *
 * 2. Hydration waits for them. `/pkg/` is served immutable while these are
 *    served `no-cache`, so on a repeat visit the wasm is ready from cache while
 *    they are still revalidating, and wasm-bindgen resolves `js_namespace`
 *    globals at call time.
 */

/** Shell scripts are the classic ones; the pkg bundle loads via dynamic import. */
const SHELL_SCRIPTS = "script[src]:not([type=module])";

test.describe("Shell critical path", () => {
  test("no shell script blocks the parser", async ({ page }) => {
    await page.goto("/");

    const scripts = await page.$$eval(SHELL_SCRIPTS, (els) =>
      els
        .map((el) => el as HTMLScriptElement)
        .filter((el) => !el.src.includes("/pkg/"))
        .map((el) => ({
          src: new URL(el.src).pathname,
          defer: el.defer,
          async: el.async,
        })),
    );

    // Guard the guard: an empty list would pass every assertion below.
    expect(scripts.length).toBeGreaterThanOrEqual(12);

    const blocking = scripts.filter((s) => !s.defer && !s.async);
    expect(
      blocking,
      `parser-blocking shell scripts: ${blocking.map((s) => s.src).join(", ")}`,
    ).toEqual([]);

    // `async` would compile away the ordering that KaTeX and Prism depend on.
    const unordered = scripts.filter((s) => s.async);
    expect(
      unordered,
      `async drops execution order: ${unordered.map((s) => s.src).join(", ")}`,
    ).toEqual([]);
  });

  test("UMD globals survive Monaco's AMD loader", async ({ page }) => {
    // The failure this catches is silent at load and only shows up when a
    // notebook renders math or a fenced code block, so assert the globals
    // directly rather than through a rendered page.
    await page.goto("/");
    await page.waitForLoadState("load");

    const globals = await page.evaluate(() => ({
      katex: typeof (window as never as Record<string, unknown>).IronpadKaTeX,
      prism: typeof (window as never as Record<string, unknown>).IronpadPrism,
      monaco: typeof (window as never as Record<string, unknown>).IronpadMonaco,
      storage: typeof (window as never as Record<string, unknown>)
        .IronpadStorage,
      executor: typeof (window as never as Record<string, unknown>)
        .IronpadExecutor,
    }));

    expect(globals).toEqual({
      katex: "object",
      prism: "object",
      monaco: "object",
      storage: "object",
      executor: "object",
    });
  });

  test("the app still hydrates behind the deferred shell", async ({ page }) => {
    // Hydration is gated on DOMContentLoaded now. If that promise never settles
    // the page serves its SSR markup and stays dead, which no static check
    // would notice. Creating a notebook needs both halves of the pairing: a
    // mounted app, and `storage.js` having run to define IndexedDB access.
    const jsErrors = trackJsErrors(page);

    // `createNotebook` already waits for `.ironpad-editor`, so reaching this
    // line at all is the hydration assertion.
    await createNotebook(page);

    expect(jsErrors).toEqual([]);
  });

  test("first paint does not wait for the shell scripts", async ({ page }) => {
    await page.goto("/");
    await page.waitForLoadState("load");

    const timing = await page.evaluate(() => {
      const paint = performance
        .getEntriesByType("paint")
        .find((p) => p.name === "first-contentful-paint");
      const shell = performance
        .getEntriesByType("resource")
        .filter(
          (r) =>
            new URL(r.name).pathname.endsWith(".js") &&
            !r.name.includes("/pkg/"),
        );
      return {
        fcp: paint ? paint.startTime : null,
        lastShellScriptEnd: shell.length
          ? Math.max(...shell.map((r) => r.responseEnd))
          : null,
        count: shell.length,
      };
    });

    // Skip rather than pass vacuously when the browser withholds the entries
    // (a warm memory cache reports no resource timing for these).
    test.skip(
      timing.fcp === null ||
        timing.lastShellScriptEnd === null ||
        timing.count === 0,
      "no paint or resource timing available in this run",
    );

    // The whole point of deferring: paint no longer sits behind the last
    // script. Before the change these were within a few ms of each other.
    expect(timing.fcp!).toBeLessThan(timing.lastShellScriptEnd!);
  });
});
