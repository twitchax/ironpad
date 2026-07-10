import { test, expect } from "@playwright/test";
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
      "/notebook/public/welcome.ironpad"
    );

    expect(jsErrors).toEqual([]);
  });

  test("embed autorun is opt-in: default idle, ?autorun=1 runs cells", async ({
    page,
  }) => {
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

  test("view page snippet popover copies both embed snippets", async ({
    page,
    context,
  }) => {
    await context.grantPermissions(["clipboard-read", "clipboard-write"]);
    const jsErrors = trackJsErrors(page);

    await page.goto("/notebook/public/welcome.ironpad");
    await expect(page.locator(".view-only-notebook")).toBeVisible({
      timeout: 30_000,
    });
    // Allow hydration to attach the click handler (suite convention).
    await page.waitForTimeout(3_000);

    await page.locator(".embed-button").click();
    const popover = page.locator(".view-only-embed-popover");
    await expect(popover).toBeVisible();

    // Both snippet styles are offered and reference this notebook.
    const snippets = popover.locator("pre");
    await expect(snippets).toHaveCount(2);
    await expect(snippets.nth(0)).toContainText(
      "/embed/public/welcome.ironpad"
    );
    await expect(snippets.nth(1)).toContainText("embed.js");
    await expect(snippets.nth(1)).toContainText(
      'data-notebook="public/welcome.ironpad"'
    );

    // The copy button actually copies the iframe snippet.
    await popover.locator("button").first().click();
    const copied = await page.evaluate(() => navigator.clipboard.readText());
    expect(copied).toContain("<iframe");
    expect(copied).toContain("/embed/public/welcome.ironpad");

    expect(jsErrors).toEqual([]);
  });
});
