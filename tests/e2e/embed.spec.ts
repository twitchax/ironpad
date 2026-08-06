import { test, expect } from "@playwright/test";
import { createNotebook } from "./helpers/session";
import { loginTestUser } from "./helpers/auth";
import { trackJsErrors } from "./helpers/errors";

/**
 * Notebook embedding (PRD-0039): chrome-less /embed routes, the embed.js
 * loader with its auto-resize protocol, and the snippet UI on view pages.
 */

const BASE = "http://localhost:3111";

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

  test("toolbar action buttons share one corner radius", async ({ page }) => {
    // Run All, Fork, and Embed sit together in the view-only toolbar and must
    // round identically. Fork/Embed once hardcoded 4px while Run All used the
    // --ip-radius-md token (6px), so they read as subtly mismatched pills.
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
    await page
      .locator('.ironpad-toolbar-dropdown-toggle[title="Notebook menu"]')
      .click();
    await page
      .locator(".ironpad-toolbar-dropdown-item", { hasText: "Share Mutable" })
      .click();
    await expect(page).toHaveURL(/\/mutable\/[a-f0-9]{16}/, {
      timeout: 30_000,
    });
    const shareId = page.url().match(/\/mutable\/([a-f0-9]{16})/)![1];

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
