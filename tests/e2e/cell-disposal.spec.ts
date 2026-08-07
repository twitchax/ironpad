import { test, expect } from "@playwright/test";
import { setCellSource } from "./helpers/monaco";
import { createNotebook, ADD_CODE } from "./helpers/session";

/**
 * Regression for the disposed-reactive-value panic when a cell is removed while
 * one of its async server round-trips (live check-on-type, or a compile/run) is
 * still in flight. The continuation used to write per-cell signals after the
 * `.await` with a plain `.set()`, so a cell disposed mid-flight left it writing
 * to a reclaimed arena slot:
 *
 *   panicked at reactive_graph/traits.rs: Tried to access a reactive value
 *   that has already been disposed.
 *
 * The panic surfaces through `console.error` (then an `unreachable` trap), so we
 * assert on the console signature directly — `trackJsErrors` deliberately drops
 * `unreachable`, which would mask this. The fix guards every post-await access
 * with `try_get_untracked()` and bails when the scope is gone.
 */

test.describe("Cell disposal during in-flight async", () => {
  test("deleting a cell mid-check does not panic on disposed reactive state", async ({
    page,
  }) => {
    test.setTimeout(240_000);

    // Capture error-level console output; the disposed panic lands here.
    const consoleErrors: string[] = [];
    page.on("console", (msg) => {
      if (msg.type() === "error") consoleErrors.push(msg.text());
    });
    // The menu delete uses a native confirm() — accept it automatically.
    page.on("dialog", (dialog) => dialog.accept());

    await createNotebook(page);
    await page
      .locator(ADD_CODE)
      .first()
      .click();
    // The card holds the editor + squiggles; the side-action rail (with the
    // menu button) is a sibling under the row, so scope the menu to the row.
    const row = page.locator(".ironpad-cell-row").first();
    const cell = row.locator(".ironpad-cell-card");
    await expect(cell.locator(".monaco-editor").first()).toBeVisible({
      timeout: 15_000,
    });

    // Arm and complete one check so the check lane is warm (subsequent checks
    // are a predictable few-second round-trip, giving a reliable race window).
    await setCellSource(page, cell, 'let x: i32 = "nope";\nx');
    await expect(cell.locator(".squiggly-error").first()).toBeVisible({
      timeout: 120_000,
    });

    // Arm a fresh check, let the 1s save debounce dispatch it, then delete the
    // cell while that check is still round-tripping.
    await setCellSource(page, cell, 'let y: i32 = "still nope";\ny');
    await page.waitForTimeout(1_300);

    await row.hover();
    await row.locator(".ironpad-side-btn[title='Cell menu']").click();
    await page
      .locator(".ironpad-cell-menu-item--danger", { hasText: "Delete" })
      .click();
    await expect(page.locator(".ironpad-cell-card")).toHaveCount(0, {
      timeout: 10_000,
    });

    // Give the in-flight check time to return onto the now-disposed cell.
    await page.waitForTimeout(15_000);

    const disposed = consoleErrors.filter((t) =>
      /already been disposed|already borrowed/i.test(t),
    );
    expect(disposed).toEqual([]);
  });
});
