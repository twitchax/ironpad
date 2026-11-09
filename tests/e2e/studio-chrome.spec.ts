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
 * The component warns to the console for each (for anchors, once it has
 * waited ~2s for a client-mounted cell list to attach), but a warning is not
 * a gate. The anchor-count assertion below is what actually catches a
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
  // Hydration: the spy is installed when the rail hydrates, not in the SSR
  // markup, so nothing tracks the scroll until the wasm has run.
  await page.waitForTimeout(3_000); // suite convention
}

/**
 * Count the rail's LIVE IntersectionObservers: +1 when one is built with the
 * cell list as its root, -1 when it disconnects. Anything else on the page
 * (Monaco, lazy images) observes against other roots and is not counted.
 */
async function countRailObservers(page: Page) {
  await page.addInitScript(() => {
    const w = window as unknown as {
      __railObservers: number;
      IntersectionObserver: typeof IntersectionObserver;
    };
    const Native = w.IntersectionObserver;
    w.__railObservers = 0;
    class Counting extends Native {
      private counted = false;
      constructor(cb: IntersectionObserverCallback, init?: IntersectionObserverInit) {
        super(cb, init);
        const root = init?.root;
        if (root instanceof Element && root.classList.contains("view-only-cells")) {
          this.counted = true;
          w.__railObservers++;
        }
      }
      disconnect() {
        if (this.counted) {
          this.counted = false;
          w.__railObservers--;
        }
        super.disconnect();
      }
    }
    w.IntersectionObserver = Counting;
  });
  return () =>
    page.evaluate(() => (window as unknown as { __railObservers: number }).__railObservers);
}

/** Scroll the cell list and require the spy to move the selection. */
async function expectSpyTracksScroll(page: Page) {
  const rows = rail(page).locator(".ip-rail-row");
  await expect(rows.first()).toHaveAttribute("aria-current", "true");
  await page.locator(".view-only-cells").evaluate((el) => el.scrollTo(0, 1500));
  await expect(rows.first()).not.toHaveAttribute("aria-current", "true", {
    timeout: 10_000,
  });
  await expect(rail(page).locator('[aria-current="true"]')).toHaveCount(1);
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
    // Exactly one row owns the selection at a time.
    await expectSpyTracksScroll(page);
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

  test("a cell that ran shows it in the rail, with a timing and Runtime totals", async ({
    page,
  }) => {
    // The rail's run state had no writer at all, so every code row sat on
    // the not-run dot forever and the Runtime totals never rendered. The
    // notebook autoruns (/public is first-party, PRD-0040), so waiting is
    // enough to drive a real run.
    test.setTimeout(300_000);
    await gotoNotebook(page);

    // Rows and anchors are both every cell in notebook order, so a cell's
    // anchor index is its row index.
    const firstCode = await page.evaluate(() =>
      Array.from(document.querySelectorAll('[id^="ip-cell-"]')).findIndex((el) =>
        el.matches(
          ".view-only-cell--frame:not(.view-only-cell--shared):not(.view-only-cell--inert):not(.view-only-cell--linux)",
        ),
      ),
    );
    expect(firstCode, "the notebook has a code cell").toBeGreaterThanOrEqual(0);

    const row = rail(page).locator(".ip-rail-row").nth(firstCode);
    await expect(row.locator(".ip-rail-dot--ran")).toHaveCount(1, { timeout: 240_000 });
    await expect(row.locator(".ip-rail-row-timing")).toHaveText(/^\d+(\.\d)?ms$/);
    await expect(rail(page).locator(".ip-rail-stats")).toBeVisible();

    // The other direction: prose rows never pick up run state.
    const markdownCells = await page.locator(".view-only-markdown").count();
    expect(markdownCells, "the notebook has prose").toBeGreaterThan(0);
    await expect(rail(page).locator(".ip-rail-dot--prose")).toHaveCount(markdownCells);
  });

  test("the rail installs exactly one observer across a client-side navigation", async ({
    page,
  }) => {
    // Home -> card click is the common way in, and it is a CLIENT-side
    // navigation: the rail's body runs before the cell list is attached, so
    // an install that resolved anchors only once found none and the spy never
    // existed. A leaked observer is the opposite failure, invisible until it
    // stacks. `page.goto` both ways would destroy the JS context and see
    // neither, so this navigates by clicking.
    const live = await countRailObservers(page);
    await gotoNotebook(page);
    expect(await live(), "a hard load installs one").toBe(1);

    await page.locator("a.ironpad-brand").click();
    await expect(page.locator(".ironpad-home")).toBeVisible({ timeout: 15_000 });
    await expect.poll(live, { message: "leaving disconnects it" }).toBe(0);

    await page.locator(`a[href="${NOTEBOOK}"]`).first().click();
    await expect(page.locator(".view-only-cell").first()).toBeVisible({ timeout: 30_000 });
    await expect.poll(live, { message: "arriving installs exactly one" }).toBe(1);
    await expectSpyTracksScroll(page);
  });

  test("the rail installs one observer in the editor's Preview, and drops it", async ({
    page,
  }) => {
    // Preview mounts the reader client-side over a live editor, the same
    // shape as a card click: no SSR anchors to find at mount time, which used
    // to log a false "no anchor" warning and install nothing.
    //
    // Install only, not the scroll: in Preview the page's content pane
    // scrolls rather than `.view-only-cells`, so the observer's root never
    // moves. That is a separate layout issue from the install.
    const anchorWarnings: string[] = [];
    page.on("console", (m) => {
      if (m.text().includes("anchor element")) anchorWarnings.push(m.text());
    });
    const live = await countRailObservers(page);
    await gotoNotebook(page);
    await page.locator(".fork-button").click();
    await expect(page).toHaveURL(/\/local\/[a-f0-9-]+/, { timeout: 15_000 });
    await expect(page.locator(".ironpad-editor")).toBeVisible({ timeout: 15_000 });
    await expect.poll(live).toBe(0);

    await page.locator('button[aria-label="Preview"]').click();
    await expect(page.locator(".view-only-cell").first()).toBeVisible({ timeout: 30_000 });
    await expect.poll(live, { message: "Preview installs exactly one" }).toBe(1);

    await page.locator('button[aria-label="Edit"]').click();
    await expect.poll(live, { message: "leaving Preview disconnects it" }).toBe(0);
    // Past the deferred install's whole wait, so a late warning would show.
    await page.waitForTimeout(2_500);
    expect(anchorWarnings).toEqual([]);
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

  test("the frame's chevron toggles the source on a code cell and a shared cell", async ({
    page,
  }) => {
    // Every framed cell kind renders its header lead (chevron, index, label)
    // and body class from one shared piece; drive two different kinds so a
    // regression in either wiring shows. shared-code ships every code cell,
    // shared ones included, collapsed.
    await page.goto("/public/shared-code");
    await expect(page.locator(".view-only-cell").first()).toBeVisible({ timeout: 30_000 });
    await page.waitForTimeout(3_000); // hydration, suite convention

    const code = page
      .locator(".view-only-cell--frame:not(.view-only-cell--shared)")
      .first();
    const shared = page.locator(".view-only-cell--shared").first();
    for (const frame of [code, shared]) {
      const body = frame.locator(".ironpad-cell-body");
      await expect(body).toHaveClass(/ironpad-cell-body--collapsed/);
      await frame.locator(".ironpad-cell-collapse-btn").click();
      await expect(body).not.toHaveClass(/ironpad-cell-body--collapsed/);
      await frame.locator(".ironpad-cell-collapse-btn").click();
      await expect(body).toHaveClass(/ironpad-cell-body--collapsed/);
    }
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
