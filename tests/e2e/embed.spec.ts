import http from "node:http";
import type { AddressInfo } from "node:net";

import { test, expect, type Page } from "@playwright/test";
import { loginTestUser } from "./helpers/auth";
import { trackJsErrors } from "./helpers/errors";
import { shareMutable } from "./helpers/mutable";
import { createNotebook } from "./helpers/session";

/**
 * Notebook embedding (PRD-0039): chrome-less /embed routes, the embed.js
 * loader with its auto-resize protocol, and the snippet UI on view pages.
 */

const BASE = "http://localhost:3111";

/**
 * Open `body` as a REAL third-party host page on its own origin (127.0.0.1 vs
 * the app's localhost). `page.route` cannot stand in for one: a fulfilled
 * https host fails on mixed content and a fulfilled http host on the
 * local-network-access check. A frame in it is never cross-origin isolated.
 * The caller closes the server in a `finally`.
 */
async function openHostPage(
  page: Page,
  body: string,
): Promise<{ close: () => Promise<void> }> {
  const server = http.createServer((_req, res) => {
    res.writeHead(200, { "content-type": "text/html; charset=utf-8" });
    res.end(`<!doctype html><html><body>${body}</body></html>`);
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const { port } = server.address() as AddressInfo;
  const close = () =>
    new Promise<void>((resolve) => {
      server.closeAllConnections();
      server.close(() => resolve());
    });
  try {
    await page.goto(`http://127.0.0.1:${port}/`);
  } catch (e) {
    await close();
    throw e;
  }
  return { close };
}

/** A host page framing `/embed/public/{name}` directly. */
function openCrossOriginEmbed(page: Page, name: string) {
  return openHostPage(
    page,
    `<h1>Host page</h1>` +
      `<iframe src="${BASE}/embed/public/${name}" style="width:100%;height:900px;border:0"></iframe>`,
  );
}

/**
 * Three embeds on one host page: every frame gets its own id, and a height
 * message one frame posts under ANOTHER frame's id must not resize it.
 */
async function expectIsolatedEmbeds(page: Page) {
  const frames = page.locator("iframe[id^='ironpad-embed-']");
  await expect(frames).toHaveCount(3, { timeout: 15_000 });
  const ids = await frames.evaluateAll((els) => els.map((e) => e.id));
  expect(new Set(ids).size, `ids: ${ids.join(", ")}`).toBe(3);

  // Let frame 2 report its own height first: a frame posts only when its
  // height changes, so a forged height that lands afterwards would stick.
  const second = frames.nth(1);
  await expect
    .poll(async () => second.evaluate((el) => el.style.height), {
      timeout: 30_000,
    })
    .not.toBe("480px");

  // A message from frame 1 naming frame 2's id passes the origin check (both
  // frames are ironpad), so only a check on the SOURCE window can refuse it.
  const first = page
    .frames()
    .find((f) => f.url().includes(`embed_id=${encodeURIComponent(ids[0])}`));
  expect(first, "frame 1 is attached").toBeTruthy();
  await first!.evaluate((id) => {
    window.parent.postMessage(
      { type: "ironpad:embed:height", id, height: 123 },
      "*",
    );
  }, ids[1]);
  await page.waitForTimeout(1_000);
  expect(await second.evaluate((el) => el.style.height)).not.toBe("123px");
}

/**
 * Drive the framed notebook: report isolation, whether the threaded-cells
 * notice rendered, and whether the first frame's chevron still toggles its
 * body (a panicked hydration leaves every handler dead).
 */
async function probeEmbedFrame(page: Page) {
  const inner = page.frameLocator("iframe");
  await expect(inner.locator(".view-only-notebook")).toBeVisible({
    timeout: 30_000,
  });
  await page.waitForTimeout(3_000); // hydration (suite convention)

  const frame = page.frames().find((f) => f.url().includes("/embed/public/"));
  const isolated = await frame!.evaluate(() => self.crossOriginIsolated);

  const cell = inner.locator(".view-only-cell--frame").first();
  const body = cell.locator(".ironpad-cell-body");
  const wasCollapsed = /ironpad-cell-body--collapsed/.test(
    (await body.getAttribute("class")) ?? "",
  );
  await cell.locator(".ironpad-cell-collapse-btn").click();
  if (wasCollapsed) {
    await expect(body).not.toHaveClass(/ironpad-cell-body--collapsed/);
  } else {
    await expect(body).toHaveClass(/ironpad-cell-body--collapsed/);
  }
  return { isolated, notice: inner.locator(".view-only-embed-notice") };
}

/**
 * Every uncaught error and panic message from the page AND its frames, raw:
 * `trackJsErrors` drops "unreachable", which is exactly what a hydration
 * panic throws.
 */
function rawErrors(page: Page): string[] {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  page.on("console", (m) => {
    if (m.type() === "error" && m.text().includes("panicked")) errors.push(m.text());
  });
  return errors;
}

test.describe("Notebook embedding", () => {
  test("embed route renders chrome-less with the ironpad badge", async ({
    page,
  }) => {
    const jsErrors = trackJsErrors(page);

    await page.goto("/embed/public/welcome.ironpad");
    await expect(page.locator(".view-only-notebook")).toBeVisible({
      timeout: 30_000,
    });

    // No app chrome inside an embed.
    await expect(page.locator(".ironpad-header")).toHaveCount(0);
    await expect(page.locator(".ironpad-status-bar")).toHaveCount(0);
    // No fork button (IndexedDB is partitioned in cross-origin iframes).
    await expect(page.locator(".fork-button")).toHaveCount(0);
    // Run All stays available: execution is opt-in but supported.
    await expect(page.locator(".run-all-button")).toBeVisible();

    // The badge links back to the canonical page.
    const badge = page.locator(".ironpad-embed-badge");
    await expect(badge).toBeVisible();
    await expect(badge).toHaveAttribute(
      "href",
      "/public/welcome"
    );

    expect(jsErrors).toEqual([]);
  });

  test("embed autorun is opt-in: default idle, ?autorun=1 runs cells", async ({
    page,
  }) => {
    // The autorun half compiles the welcome notebook's cells; a fully cold
    // cache needs minutes, and the default 30s TEST timeout silently capped
    // the generous expect timeout below (the test only ever passed warm).
    test.setTimeout(600_000);

    // Default: an embed does NOT auto-run (readers of a third-party page
    // didn't choose ironpad). No timing badge appears without a click.
    await page.goto("/embed/public/welcome.ironpad");
    await expect(page.locator(".view-only-notebook")).toBeVisible({
      timeout: 30_000,
    });
    await page.waitForTimeout(3_000);
    await expect(page.locator(".view-only-timing-badge")).toHaveCount(0);

    // With the embedder's opt-in (?autorun=1, forwarded by embed.js from
    // data-autorun), cells run on load.
    await page.goto("/embed/public/welcome.ironpad?autorun=1");
    await expect(page.locator(".view-only-timing-badge").first()).toBeVisible({
      timeout: 480_000,
    });
  });

  test("embed.js script snippet mounts an auto-resizing iframe", async ({
    page,
  }) => {
    // A minimal third-party host page using snippet style 1 (self-invoking
    // script tag). setContent gives it a non-ironpad origin, so this also
    // exercises the cross-origin message validation.
    await page.setContent(`
      <!doctype html>
      <html><body>
        <h1>Host page</h1>
        <script src="${BASE}/embed.js"
                data-notebook="public/welcome.ironpad"></script>
      </body></html>
    `);

    const frame = page.locator("iframe[id^='ironpad-embed-']");
    await expect(frame).toBeVisible({ timeout: 15_000 });

    // The notebook renders inside the frame.
    const inner = page.frameLocator("iframe[id^='ironpad-embed-']");
    await expect(inner.locator(".view-only-notebook")).toBeVisible({
      timeout: 30_000,
    });

    // The height protocol replaces the 480px placeholder once the frame
    // reports its rendered size.
    await expect
      .poll(async () => frame.evaluate((el) => el.style.height), {
        timeout: 15_000,
      })
      .not.toBe("480px");
  });

  test("embed.js mounts multiple placeholder divs independently", async ({
    page,
  }) => {
    await page.setContent(`
      <!doctype html>
      <html><body>
        <div class="ironpad-embed" data-notebook="public/welcome.ironpad"></div>
        <div class="ironpad-embed" data-notebook="public/tutorial.ironpad"></div>
        <script src="${BASE}/embed.js"></script>
      </body></html>
    `);

    const frames = page.locator("iframe[id^='ironpad-embed-']");
    await expect(frames).toHaveCount(2, { timeout: 15_000 });

    // Distinct ids → distinct resize channels.
    const ids = await frames.evaluateAll((els) => els.map((e) => e.id));
    expect(new Set(ids).size).toBe(2);
  });

  test("embed.js script tags never share an id or resize each other", async ({
    page,
  }) => {
    test.setTimeout(90_000);
    // Each style-1 tag runs its own copy of the loader, so a per-script
    // counter starts at 0 in every one of them. The host freezes its own
    // clock first, so all three tags run "in the same millisecond", which is
    // the case that collided (4 of 15 real loads with two async tags).
    const host = await openHostPage(
      page,
      `<script>const frozen = Date.now(); Date.now = () => frozen;</script>
       <script src="${BASE}/embed.js" data-notebook="public/welcome" async></script>
       <script src="${BASE}/embed.js" data-notebook="public/tutorial" async></script>
       <script src="${BASE}/embed.js" data-notebook="public/welcome" async></script>`,
    );
    try {
      await expectIsolatedEmbeds(page);
    } finally {
      await host.close();
    }
  });

  test("a cross-origin embed of a rayon notebook explains itself and stays live", async ({
    page,
  }) => {
    // The notice is computed from `crossOriginIsolated`, which SSR cannot see.
    // Rendering it straight from that value put a `<div>` where the server
    // wrote nothing, and hydration panicked on the mismatch, killing every
    // handler on the page. Same-origin tests are isolated, so this only shows
    // under a real third-party host.
    const errors = rawErrors(page);
    const host = await openCrossOriginEmbed(page, "mandelbrot");
    try {
      const { isolated, notice } = await probeEmbedFrame(page);
      expect(isolated).toBe(false);
      await expect(notice).toBeVisible();
      expect(errors).toEqual([]);
    } finally {
      await host.close();
    }
  });

  test("a cross-origin embed with no threaded cells shows no notice", async ({
    page,
  }) => {
    const errors = rawErrors(page);
    const host = await openCrossOriginEmbed(page, "welcome");
    try {
      const { isolated, notice } = await probeEmbedFrame(page);
      expect(isolated).toBe(false);
      await expect(notice).toHaveCount(0);
      expect(errors).toEqual([]);
    } finally {
      await host.close();
    }
  });

  test("toolbar action buttons share one shape", async ({ page }) => {
    // Run All, Fork, and Embed sit together in the view-only toolbar and must
    // round AND size identically. Fork/Embed once hardcoded 4px while Run All
    // used the --ip-radius-md token (6px); separately the three carried three
    // different paddings and font sizes, which reads as sloppiness the moment
    // they sit side by side. Both now come from the ip-button-shape mixin.
    await page.goto("/public/welcome");
    await expect(page.locator(".view-only-notebook")).toBeVisible({
      timeout: 30_000,
    });

    const runAll = page.locator(".run-all-button");
    const fork = page.locator(".fork-button");
    const embed = page.locator(".embed-button");
    await expect(runAll).toBeVisible();
    await expect(fork).toBeVisible();
    await expect(embed).toBeVisible();

    // Anchor on Run All's computed radius, then require the other two to match.
    const radius = await runAll.evaluate(
      (el) => getComputedStyle(el).borderRadius
    );
    expect(radius).toBe("6px");
    await expect(fork).toHaveCSS("border-radius", radius);
    await expect(embed).toHaveCSS("border-radius", radius);

    // ...and one height. Measured, not asserted from CSS: padding, font-size,
    // and line-height all feed it, and only the box tells you they agree.
    const heights = await Promise.all(
      [runAll, fork, embed].map((b) =>
        b.evaluate((el) => Math.round(el.getBoundingClientRect().height))
      )
    );
    expect(new Set(heights).size, `heights differ: ${heights.join(", ")}`).toBe(
      1
    );
    expect(heights[0]).toBeGreaterThanOrEqual(34);

    // The Cached/Fresh toggle sits in the same row and must match them. It
    // shares a class with the HEADER theme toggle, which is deliberately
    // shorter, so this is the half of that split that must not shrink.
    const cache = await page
      .locator(".view-only-toolbar .ironpad-theme-toggle-segment")
      .first()
      .evaluate((el) => Math.round(el.getBoundingClientRect().height));
    expect(cache).toBe(heights[0]);

    // ...and the header's copy of that class IS shorter, because a 48px
    // header does not want a full-height control in it. Asserted by
    // measurement: the rule that shrinks it sets `height`, which a stray
    // `min-height` on the shared class silently beat once.
    const header = await page
      .locator(".ironpad-header .ironpad-theme-toggle-segment")
      .first()
      .evaluate((el) => Math.round(el.getBoundingClientRect().height));
    expect(header).toBeLessThan(heights[0]);
  });

  test("view page snippet popover copies both embed snippets", async ({
    page,
    context,
  }) => {
    await context.grantPermissions(["clipboard-read", "clipboard-write"]);
    const jsErrors = trackJsErrors(page);

    await page.goto("/public/welcome");
    await expect(page.locator(".view-only-notebook")).toBeVisible({
      timeout: 30_000,
    });
    // Allow hydration to attach the click handler (suite convention).
    await page.waitForTimeout(3_000);

    await page.locator(".embed-button").click();
    const popover = page.locator(".view-only-embed-popover");
    await expect(popover).toBeVisible();

    // Both snippet styles are offered and reference this notebook. The spec
    // is extension-less (PRD-0048); the closing quote pins that no .ironpad
    // suffix sneaks back in.
    const snippets = popover.locator("pre");
    await expect(snippets).toHaveCount(2);
    await expect(snippets.nth(0)).toContainText('/embed/public/welcome"');
    await expect(snippets.nth(1)).toContainText("embed.js");
    await expect(snippets.nth(1)).toContainText(
      'data-notebook="public/welcome"'
    );

    // The copy button actually copies the iframe snippet.
    await popover.locator("button").first().click();
    const copied = await page.evaluate(() => navigator.clipboard.readText());
    expect(copied).toContain("<iframe");
    expect(copied).toContain('/embed/public/welcome"');
    // Without it a cross-origin frame's clipboard write is refused, and every
    // output panel's Copy button inside the embed copies nothing.
    expect(copied).toContain('allow="clipboard-write"');

    expect(jsErrors).toEqual([]);
  });

  test("a published notebook embeds and resolves through oEmbed (PRD-0057)", async ({
    page,
    request,
  }) => {
    test.setTimeout(120_000);
    const BASE = "http://localhost:3111";

    // Publish a notebook to get a live /mutable id.
    await loginTestUser(page, "alice");
    await createNotebook(page);
    await page.locator(".ironpad-notebook-title--editable").click();
    const input = page.locator(".ironpad-header-title-input");
    await input.fill("Embeddable Published");
    await input.press("Enter");
    const shareId = await shareMutable(page);

    // oEmbed maps the canonical URL to the mutable embed route.
    const oembed = await request.get(
      `${BASE}/oembed?url=${encodeURIComponent(`${BASE}/mutable/${shareId}`)}`,
    );
    expect(oembed.status()).toBe(200);
    const body = await oembed.json();
    expect(body.html).toContain(`${BASE}/embed/mutable/${shareId}`);
    expect(body.title).toBe("Embeddable Published");

    // The reader page advertises the endpoint in its raw SSR body.
    const raw = await (await request.get(`${BASE}/mutable/${shareId}`)).text();
    expect(raw).toMatch(/<link[^>]+type="application\/json\+oembed"[^>]*>/i);

    // The embed route renders chrome-less, badge linking to the canonical
    // page, and never autoruns (idle status, published-author content).
    await page.goto(`/embed/mutable/${shareId}`);
    await expect(page.locator(".view-only-notebook")).toBeVisible({
      timeout: 30_000,
    });
    await expect(page.locator(".ironpad-header")).toHaveCount(0);
    await expect(page.locator(".ironpad-embed-badge")).toHaveAttribute(
      "href",
      `/mutable/${shareId}`,
    );
    await page.waitForTimeout(2_000);
    await expect(page.locator(".view-only-cell-status--running")).toHaveCount(
      0,
    );
  });
});
