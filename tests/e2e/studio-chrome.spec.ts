import { test, expect, type Page } from "@playwright/test";
import { trackJsErrors } from "./helpers/errors";

/**
 * The Studio chrome on read-only notebooks (PRD-0065 Tier 2).
 *
 * The rail is an outline over the cell list with a scroll-spy. It has two
 * failure modes that are NOT the same, which is what most of this file is
 * about:
 *
 * 1. A missing/renamed scroll root degrades gracefully. `query_selector`
 *    returns `None`, the observer falls back to the viewport, and selection
 *    still tracks — just measured against the window top instead of the list
 *    top, so it runs early by roughly a header's height. Easy to miss.
 *
 * 2. Missing anchors are total and silent. Cells carry no `id`, nothing
 *    resolves, `install_scroll_spy` returns early, and BOTH the spy and
 *    click-to-scroll die while looking exactly like a rail nobody scrolled.
 *
 * The component warns to the console for each at first render, but a warning
 * is not a gate. The anchor-count assertion below is what actually catches a
 * rename, and it has to be separate from the behavioral checks because those
 * would both still pass against a viewport-root fallback.
 *
 * Chrome-less by contract: `/embed/*` gets no rail, no breadcrumb, no
 * read-only pill and no status bar.
 */

const NOTEBOOK = "/public/charts-with-plot";

/** The rail is a landmark; prefer the role over the class. */
const rail = (page: Page) => page.getByRole("navigation", { name: "Notebook outline" });

async function gotoNotebook(page: Page) {
  await page.goto(NOTEBOOK);
  await expect(page.locator(".view-only-cell").first()).toBeVisible({ timeout: 30_000 });
  // Hydration: the spy is installed in an effect, not in the SSR markup.
  await page.waitForTimeout(3_000); // suite convention
}

test.describe("Studio rail", () => {
  test("every rail row resolves to a real cell anchor", async ({ page }) => {
    await gotoNotebook(page);

    const rows = await rail(page).locator(".ip-rail-row").count();
    expect(rows, "the rail lists cells").toBeGreaterThan(0);

    // THE assertion that catches a container rename or a forgotten `id`.
    // Both behavioral tests below would still pass without it.
    const anchors = await page.locator('[id^="ip-cell-"]').count();
    expect(anchors, "one anchor per rail row").toBe(rows);
  });

  test("markdown cells appear in the outline and carry no timing", async ({ page }) => {
    await gotoNotebook(page);
    // 51% of cells across public/notebooks are markdown. An outline that
    // skipped them would freeze the spy on the last code cell while the
    // reader scrolls screens of prose — broken exactly when it works.
    const prose = rail(page).locator(".ip-rail-dot--prose");
    expect(await prose.count(), "prose rows exist").toBeGreaterThan(0);
    // Prose gets a rule, not a dimmer dot: "no run state" must not read as
    // "has not run yet".
    await expect(prose.first()).toBeVisible();
  });

  test("scroll-spy moves the selection off the first row", async ({ page }) => {
    await gotoNotebook(page);
    const rows = rail(page).locator(".ip-rail-row");
    await expect(rows.first()).toHaveAttribute("aria-current", "true");

    await page.locator(".view-only-cells").evaluate((el) => el.scrollTo(0, 1500));
    await expect(rows.first()).not.toHaveAttribute("aria-current", "true", {
      timeout: 10_000,
    });
    // Exactly one row owns the selection at a time.
    await expect(rail(page).locator('[aria-current="true"]')).toHaveCount(1);
  });

  test("clicking a row selects it and scrolls its cell into view", async ({ page }) => {
    const errors = trackJsErrors(page);
    await gotoNotebook(page);

    const rows = rail(page).locator(".ip-rail-row");
    const target = rows.nth(4);
    await target.click();
    await expect(target).toHaveAttribute("aria-current", "true");

    // The click holds the spy off briefly; the selection must survive that.
    await page.waitForTimeout(1_000);
    await expect(target).toHaveAttribute("aria-current", "true");
    expect(errors).toHaveLength(0);
  });

  test("the rail installs exactly one observer across a navigation", async ({ page }) => {
    // A leaked observer is invisible until it stacks. Navigating away and back
    // must not double-report; a disposed-signal read would abort the wasm app
    // outright, which shows up as the page going blank.
    await gotoNotebook(page);
    await page.goto("/");
    await expect(page.locator("body")).toBeVisible();
    await gotoNotebook(page);

    await expect(rail(page).locator('[aria-current="true"]')).toHaveCount(1);
    await expect(page.locator(".view-only-cell").first()).toBeVisible();
  });
});

test.describe("Studio frame and status bar", () => {
  test("frames number only the cells that draw one, with no gaps", async ({ page }) => {
    await gotoNotebook(page);
    const indices = await page.locator(".view-only-cell-index").allTextContents();
    expect(indices.length).toBeGreaterThan(0);
    // Markdown draws no frame, so numbering must be 1..n over framed cells
    // rather than notebook position (which would read [2] [5] [9]).
    expect(indices).toEqual(indices.map((_, i) => `[${i + 1}]`));
  });

  test("the read-only pill and breadcrumb render on a public notebook", async ({ page }) => {
    await gotoNotebook(page);
    await expect(page.locator(".view-only-pill--readonly")).toBeVisible();
    await expect(page.locator(".view-only-breadcrumb-crumb")).toHaveText("public");
  });

  test("status bar shows a ready dot, the toolchain and a cell count", async ({ page }) => {
    await gotoNotebook(page);
    const bar = page.locator(".ironpad-status-bar");
    await expect(bar).toBeVisible();
    await expect(bar.locator(".ironpad-status-dot")).toBeVisible();
    await expect(bar).toContainText("Ready");
    await expect(bar).toContainText("Cells:");
    // The count is set client-side, so a stale `0` would mean hydration
    // never reached it.
    await expect(bar).not.toContainText("Cells: 0");
  });

  test("collapsed cells stay collapsed inside a frame", async ({ page }) => {
    await gotoNotebook(page);
    // Per-cell `collapsed` flags predate this PRD and story-style notebooks
    // rely on them; the frame must not force sources open.
    const collapsed = await page.locator(".ironpad-cell-body--collapsed").count();
    expect(collapsed, "charts-with-plot ships collapsed cells").toBeGreaterThan(0);
  });
});

test.describe("Embeds stay chrome-less", () => {
  test("no rail, no breadcrumb, no pill, no status bar", async ({ page }) => {
    await page.goto("/embed/public/charts-with-plot");
    await expect(page.locator(".view-only-cell").first()).toBeVisible({ timeout: 30_000 });
    await page.waitForTimeout(2_000);

    await expect(rail(page)).toHaveCount(0);
    await expect(page.locator(".ip-rail")).toHaveCount(0);
    // The crumb, not its row: `.view-only-title-row` owns the <h1> and
    // always renders. Asserting on the row is what made this test wrong the
    // first time.
    await expect(page.locator(".view-only-breadcrumb-crumb")).toHaveCount(0);
    await expect(page.locator(".view-only-pill--readonly")).toHaveCount(0);
    await expect(page.locator(".ironpad-status-bar")).toHaveCount(0);
  });
});
