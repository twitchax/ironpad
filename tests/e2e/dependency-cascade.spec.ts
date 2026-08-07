import { test, expect } from "@playwright/test";
import { setCellSource } from "./helpers/monaco";
import { ADD_CODE } from "./helpers/session";

/**
 * PRD-0060: dependency-aware run cascade + continue-past-failures.
 *
 * A cell consumes upstream outputs only through the injected `cellN`
 * bindings, so running a cell cascades exactly its transitive dependencies;
 * a failure in a run queue drops only the failed cell's true dependents
 * (marked blocked) while independent cells keep running.
 */

async function newNotebookWithCells(
  page: import("@playwright/test").Page,
  sources: string[],
) {
  await page.goto("/");
  await expect(page.locator(".ironpad-home")).toBeVisible();
  await page.waitForTimeout(3_000); // hydration (suite convention)
  await page.locator("button", { hasText: "New Notebook" }).click();
  await expect(page).toHaveURL(/\/local\/[a-f0-9-]+/);
  await expect(page.locator(".ironpad-editor")).toBeVisible();

  for (let i = 0; i < sources.length; i += 1) {
    // Each click appends a fresh Code cell at the end (the markdown variant
    // shares the base class, so select the code button explicitly).
    await page
      .locator(ADD_CODE)
      .last()
      .click();
    const cell = page.locator(".ironpad-cell-card").nth(i);
    await expect(cell.locator(".monaco-editor").first()).toBeVisible({
      timeout: 15_000,
    });
    await setCellSource(page, cell, sources[i]);
  }
  return page.locator(".ironpad-cell-card");
}

test.describe("Dependency-aware cascade (PRD-0060)", () => {
  test("running an independent cell does not compile upstream cells", async ({
    page,
  }) => {
    test.setTimeout(240_000);

    const cells = await newNotebookWithCells(page, [
      'CellOutput::text("upstream, never consumed")',
      "40 + 2",
    ]);

    // Run only the second cell: it references no cellN slot, so nothing
    // cascades — the first cell must stay idle from start to finish.
    await page.locator('button[title="Run cell"]').nth(1).click();
    await expect(
      cells.nth(1).locator(".ironpad-cell-status--success"),
    ).toBeVisible({ timeout: 120_000 });
    await expect(
      cells.nth(0).locator(".ironpad-cell-status--idle"),
    ).toBeVisible();
  });

  test("running a consumer cascades exactly its dependency chain", async ({
    page,
  }) => {
    test.setTimeout(300_000);

    const cells = await newNotebookWithCells(page, [
      "7u32",
      'CellOutput::text("independent bystander")',
      "cell0 * 6",
    ]);

    // Run the consumer: slot 0 must cascade first; the bystander at slot 1
    // is not consumed and must stay idle.
    await page.locator('button[title="Run cell"]').nth(2).click();
    await expect(
      cells.nth(2).locator(".ironpad-cell-status--success"),
    ).toBeVisible({ timeout: 240_000 });
    await expect(
      cells.nth(0).locator(".ironpad-cell-status--success"),
    ).toBeVisible();
    await expect(
      cells.nth(1).locator(".ironpad-cell-status--idle"),
    ).toBeVisible();
    await expect(
      cells.nth(2).locator(".ironpad-output-display-text"),
    ).toContainText("42");
  });

  test("Run All continues past a failure; only true dependents block", async ({
    page,
  }) => {
    test.setTimeout(300_000);

    const cells = await newNotebookWithCells(page, [
      "this does not compile",
      "cell0", // consumes the failing cell: must block, not run
      '"independent survivor".to_string()', // must still run
    ]);

    await page.locator(".ironpad-run-all-button").click();

    // The failing cell errors; the independent cell still executes; the
    // dependent is dropped with an explicit blocked status instead of
    // running against a missing input.
    await expect(
      cells.nth(0).locator(".ironpad-cell-status--error"),
    ).toBeVisible({ timeout: 240_000 });
    await expect(
      cells.nth(2).locator(".ironpad-cell-status--success"),
    ).toBeVisible({ timeout: 240_000 });
    await expect(
      cells.nth(1).locator(".ironpad-cell-status--blocked"),
    ).toBeVisible({ timeout: 30_000 });

    // A rerun is a fresh attempt: edit the blocked cell to drop the failing
    // dependency and run it — it must land on Success and STAY there (the
    // stale blocked-by entry once snapped it back to Blocked over fresh
    // output).
    await setCellSource(page, cells.nth(1), "2 + 2");
    await page.locator('button[title="Run cell"]').nth(1).click();
    await expect(
      cells.nth(1).locator(".ironpad-cell-status--success"),
    ).toBeVisible({ timeout: 120_000 });
    await expect(
      cells.nth(1).locator(".ironpad-cell-status--blocked"),
    ).toHaveCount(0);
  });
});
