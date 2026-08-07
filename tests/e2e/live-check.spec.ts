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
});
