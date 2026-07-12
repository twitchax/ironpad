import { test, expect, Page, Locator } from "@playwright/test";
import { setCellSource } from "./helpers/monaco";
import { trackJsErrors } from "./helpers/errors";
import { createNotebook } from "./helpers/session";

/**
 * E2E coverage for PRD-0046 shared-cell live check: typing an error into a
 * shared (amber) cell paints an inline marker in THAT cell, at cell-local
 * lines, without anything running, and fixing the code clears it.
 *
 * Contention note: a check that exceeds its budget returns TimedOut, and by
 * design the client waits for the NEXT EDIT to retry (PRD-0045 contract).
 * Under full-suite load the first rounds can time out, so the waits below
 * nudge the editor periodically — exactly what a real typist does — while
 * keeping the assertions strict.
 */

const BROKEN = 'pub fn double(x: &str) -> u32 {\n    x * 2\n}';
const FIXED = "pub fn double(x: u32) -> u32 {\n    x * 2\n}";

/// Wait for `probe` to pass, re-setting the cell source (with a comment
/// suffix that keeps the code's meaning) between attempts to re-trigger the
/// debounced live check after TimedOut/Skipped rounds.
async function expectWithNudge(
  page: Page,
  cell: Locator,
  source: string,
  probe: () => Promise<void>
) {
  const attempts = 4;
  for (let i = 0; ; i++) {
    try {
      await probe();
      return;
    } catch (e) {
      if (i >= attempts - 1) throw e;
      await setCellSource(page, cell, `${source}\n// retry ${i}`);
    }
  }
}

test.describe("Shared-cell live check (PRD-0046)", () => {
  test("typing an error into a shared cell paints a squiggle; fixing clears it", async ({
    page,
  }) => {
    test.setTimeout(600_000);
    const jsErrors = trackJsErrors(page);

    await createNotebook(page);
    await page
      .locator(".ironpad-add-cell-btn", { hasText: "+ Code" })
      .first()
      .click();
    const cell = page.locator(".ironpad-cell-card").first();
    await expect(cell.locator(".monaco-editor").first()).toBeVisible({
      timeout: 15_000,
    });

    // Valid shared code first, so the shared toggle lands on a clean cell.
    await setCellSource(page, cell, FIXED);
    // Wait out the save debounce (dirty dot clears) before toggling shared,
    // so the model holds the source the assembly will use.
    await expect(cell.getByText("Code ●")).toHaveCount(0, {
      timeout: 15_000,
    });

    // Mark it shared via the cell menu.
    await cell.locator("..").locator('button[title="Cell menu"]').click();
    await page
      .locator(".ironpad-cell-menu-item", { hasText: "Mark as Shared" })
      .click();
    await expect(page.locator(".ironpad-cell-card--shared")).toHaveCount(1);

    // Break the shared code: `x * 2` on a &str is a guaranteed type error.
    // The save debounce fires the shared check (a cargo check round-trip).
    await setCellSource(page, cell, BROKEN);
    await expectWithNudge(page, cell, BROKEN, async () => {
      await expect(cell.locator(".squiggly-error").first()).toBeVisible({
        timeout: 60_000,
      });
    });

    // Nothing ran: shared cells have no run button, no status, no output.
    await expect(cell.locator(".ironpad-cell-status--success")).toHaveCount(0);
    await expect(cell.locator(".ironpad-output-display-text")).toHaveCount(0);

    // Fix the code — markers clear once a clean check round returns.
    await setCellSource(page, cell, FIXED);
    await expectWithNudge(page, cell, FIXED, async () => {
      await expect(cell.locator(".squiggly-error")).toHaveCount(0, {
        timeout: 60_000,
      });
    });

    expect(jsErrors).toEqual([]);
  });
});
