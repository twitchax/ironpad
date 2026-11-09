import { test, expect } from "@playwright/test";
import { setCellSource } from "./helpers/monaco";
import { trackJsErrors } from "./helpers/errors";
import { createNotebook, ADD_CODE } from "./helpers/session";

/**
 * E2E coverage for PRD-0045 live check-on-type: diagnostics appear as inline
 * markers after the save debounce, without the cell ever being run, and
 * clear again when the code is fixed.
 */

test.describe("Live check-on-type (PRD-0045)", () => {
  test("typing a type error paints a squiggle without running; fixing clears it", async ({
    page,
  }) => {
    test.setTimeout(240_000);
    const jsErrors = trackJsErrors(page);

    await createNotebook(page);
    await page
      .locator(ADD_CODE)
      .first()
      .click();
    const cell = page.locator(".ironpad-cell-card").first();
    await expect(cell.locator(".monaco-editor").first()).toBeVisible({
      timeout: 15_000,
    });

    // A guaranteed type error. The 1s save debounce fires the check; the
    // check itself is a cargo check round-trip (warm: seconds).
    await setCellSource(page, cell, 'let x: i32 = "not a number";\nx');

    await expect(cell.locator(".squiggly-error").first()).toBeVisible({
      timeout: 120_000,
    });

    // The cell was never run: no output, no success badge.
    await expect(cell.locator(".ironpad-cell-status--success")).toHaveCount(0);
    await expect(cell.locator(".ironpad-output-display-text")).toHaveCount(0);

    // Fix the code — markers clear on the next check round.
    await setCellSource(page, cell, "let x: i32 = 42;\nx");
    await expect(cell.locator(".squiggly-error")).toHaveCount(0, {
      timeout: 120_000,
    });

    expect(jsErrors).toEqual([]);
  });

  test("a reference to an unrun upstream slot is not checked (review soc-app-client-4)", async ({
    page,
  }) => {
    // The eligibility gate skipped only when EVERY upstream type was empty,
    // while the scaffold binds a slot only when its own type is known. Run
    // just the first cell, and a third cell reading the unrun second one was
    // checked against a scaffold with no `cell1` binding: a false E0425
    // squiggle on code that runs fine (Run cascades the second cell first).
    test.setTimeout(300_000);
    const jsErrors = trackJsErrors(page);

    await createNotebook(page);
    for (let i = 0; i < 3; i++) {
      await page.locator(ADD_CODE).last().click();
    }
    const cells = page.locator(".ironpad-cell-card");
    await expect(cells).toHaveCount(3);
    await expect(cells.nth(2).locator(".monaco-editor").first()).toBeVisible({
      timeout: 15_000,
    });
    await setCellSource(page, cells.nth(0), "10u32");
    await setCellSource(page, cells.nth(1), "cell0 + 1");
    await setCellSource(page, cells.nth(2), "5u32");

    await page.locator(".ironpad-cell-row").first().locator('button[title="Run cell"]').click();
    await expect(cells.nth(0).locator(".ironpad-cell-status--success")).toBeVisible({
      timeout: 180_000,
    });

    // Watch the checks themselves: a check that was sent but has not come
    // back yet paints nothing, so "no squiggle" alone cannot tell a skipped
    // check from a slow one.
    const checked: string[] = [];
    page.on("request", (req) => {
      if (!req.url().includes("check_cell")) return;
      const body = req.postData() ?? "";
      let decoded = body;
      try {
        decoded = decodeURIComponent(body.replace(/\+/g, " "));
      } catch {}
      checked.push(decoded);
    });

    // Negative FIRST: a skipped check leaves earlier markers in place, so a
    // positive control run before this would make it fail for the wrong
    // reason.
    await setCellSource(page, cells.nth(2), "cell1 + 1");
    await page.waitForTimeout(5_000);
    expect(checked.filter((b) => b.includes("cell1 + 1"))).toEqual([]);
    await expect(cells.nth(2).locator(".squiggly-error")).toHaveCount(0);

    // Positive control: checks are live for this cell, and the request
    // watcher sees them.
    await setCellSource(page, cells.nth(2), 'let x: i32 = "not a number";\nx');
    await expect(cells.nth(2).locator(".squiggly-error").first()).toBeVisible({
      timeout: 120_000,
    });
    expect(checked.some((b) => b.includes("not a number"))).toBe(true);

    expect(jsErrors).toEqual([]);
  });
});

