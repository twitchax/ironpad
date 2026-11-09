import { test, expect } from "@playwright/test";
import { fixtureCell, notebookFixture, shareFixture } from "./helpers/share";

/**
 * Untrusted markup in a shared notebook must not be able to cover the
 * reader's page, by any of the three routes that survive the style allowlist
 * on their own:
 *
 * - a `class` naming one of the app's own `position:fixed` rules
 *   (`.ironpad-history-overlay` is fixed, inset 0, z-index 1000);
 * - `transform: scale(...)`, which paints far outside the element's box;
 * - negative margins on an oversized block.
 *
 * The first is closed by the sanitizer's class allowlist, the other two by
 * `contain: paint` on the containers markup is injected into. The anchors are
 * still in the DOM afterwards; what is asserted is that none of them is what a
 * click at the header or any viewport corner lands on.
 */

const EVIL = "https://evil.example/";

test("injected markup cannot cover the reader's page", async ({ page, browser }) => {
  test.setTimeout(120_000);

  const fixture = notebookFixture("Containment", [
    fixtureCell(
      "md_class",
      0,
      "Markdown",
      `Class <a href="${EVIL}class" class="ironpad-history-overlay">CLASS</a>`,
    ),
    fixtureCell(
      "md_transform",
      1,
      "Markdown",
      `Transform <a href="${EVIL}transform" style="display:block;width:40px;height:40px;` +
        `transform:scale(80);background:rgb(255,0,0)">T</a>`,
    ),
    fixtureCell(
      "md_margin",
      2,
      "Markdown",
      `Margin <a href="${EVIL}margin" style="display:block;width:6000px;height:6000px;` +
        `margin:-3000px;background:rgb(0,0,255)">M</a>`,
    ),
    // Control: an unknown class reaches no rule at all, so this anchor stays a
    // small inline link both before and after the fix.
    fixtureCell(
      "md_control",
      3,
      "Markdown",
      `Control <a href="${EVIL}control" class="unknown-class">C</a>`,
    ),
  ]);
  const url = await shareFixture(page, fixture);

  const anon = await browser.newContext();
  try {
    const reader = await anon.newPage();
    await reader.goto(url);
    await expect(reader.locator(".view-only-notebook")).toBeVisible({
      timeout: 30_000,
    });
    // Every payload rendered (so the assertions below are not vacuous).
    for (const kind of ["class", "transform", "margin", "control"]) {
      await expect(reader.locator(`a[href="${EVIL}${kind}"]`)).toHaveCount(1);
    }

    const hits = await reader.evaluate((evil) => {
      const w = window.innerWidth;
      const h = window.innerHeight;
      const header = document.querySelector("header")?.getBoundingClientRect();
      const points: Array<[string, number, number]> = [
        ["top-left", 2, 2],
        ["top-right", w - 3, 2],
        ["bottom-left", 2, h - 3],
        ["bottom-right", w - 3, h - 3],
      ];
      if (header) {
        points.push(["header", header.left + header.width / 2, header.top + header.height / 2]);
      }
      return points.map(([name, x, y]) => {
        const el = document.elementFromPoint(x, y);
        const a = el?.closest(`a[href^="${evil}"]`);
        return { name, hit: a?.getAttribute("href") ?? null };
      });
    }, EVIL);

    for (const { name, hit } of hits) {
      expect(hit, `a click at ${name} lands on injected markup`).toBeNull();
    }

    // The control proves the class is what did it: an unknown class gives an
    // ordinary inline link, not a page-sized box.
    const control = await reader
      .locator(`a[href="${EVIL}control"]`)
      .evaluate((el) => el.getBoundingClientRect().width);
    expect(control).toBeLessThan(100);
  } finally {
    await anon.close();
  }
});
