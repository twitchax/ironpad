import { test, expect, Page } from "@playwright/test";
import { setCellSource } from "./helpers/monaco";
import { createNotebook } from "./helpers/session";

/**
 * Review bug-compiler-6: the compile pipeline retries TRANSPORT failures
 * only. A server-side refusal (at capacity, rate limited, timed out, an
 * invalid request) is a verdict, and re-asking it ran each one up to three
 * times, costing a build token and up to 180s of queueing per attempt.
 */

const CELL_SOURCE = 'CellOutput::text(format!("{}", 41 + 1))';

/** Add a cell with `CELL_SOURCE` and click Run. */
async function addCellAndRun(page: Page): Promise<void> {
  await page.locator(".ironpad-add-cell-btn").first().click();
  const cell = page.locator(".ironpad-cell-card").first();
  await expect(cell.locator(".monaco-editor").first()).toBeVisible({
    timeout: 15_000,
  });
  await setCellSource(page, cell, CELL_SOURCE);
  await page.locator('button[title="Run cell"]').first().click();
}

test.describe("Compile retry policy (review bug-compiler-6)", () => {
  test("a server error is shown once, never retried", async ({ page }) => {
    test.setTimeout(120_000);
    await createNotebook(page);

    let posts = 0;
    await page.route("**/compile_cell*", async (route) => {
      posts += 1;
      await route.fulfill({
        status: 500,
        contentType: "text/plain",
        body: "ServerError|boom",
      });
    });

    await addCellAndRun(page);

    const cell = page.locator(".ironpad-cell-card").first();
    await expect(cell.locator(".ironpad-cell-status--error")).toBeVisible({
      timeout: 30_000,
    });
    await expect(cell.locator(".ironpad-error-panel")).toContainText("boom");
    // The old policy's retries back off 1.5s and then 3s; wait past both so
    // a retry would have landed before the count is read.
    await page.waitForTimeout(6_000);
    expect(posts).toBe(1);
  });

  test("a dropped connection is retried and the run succeeds", async ({
    page,
  }) => {
    test.setTimeout(600_000);
    await createNotebook(page);

    // Positive control: without it, the test above would also pass against
    // a pipeline that never retries anything.
    let posts = 0;
    await page.route("**/compile_cell*", async (route) => {
      posts += 1;
      if (posts === 1) {
        await route.abort("connectionreset");
      } else {
        await route.continue();
      }
    });

    await addCellAndRun(page);

    const cell = page.locator(".ironpad-cell-card").first();
    await expect(cell.locator(".ironpad-cell-status--success")).toBeVisible({
      timeout: 300_000,
    });
    await expect(cell.locator(".ironpad-output-display-text")).toContainText(
      "42"
    );
    expect(posts).toBe(2);
  });
});
