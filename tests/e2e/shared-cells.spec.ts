import { test, expect } from "@playwright/test";
import { setCellSource } from "./helpers/monaco";
import { trackJsErrors } from "./helpers/errors";
import { createNotebook } from "./helpers/session";

/**
 * E2E coverage for PRD-0044 shared cells: a cell marked shared folds into
 * shared.rs (consumable as shared::*), renders with the amber chrome, never
 * runs, and is skipped by Run All.
 */

test.describe("Shared cells (PRD-0044)", () => {
  test("marked cell feeds shared::fn to a later cell and never runs itself", async ({
    page,
  }) => {
    test.setTimeout(180_000);
    const jsErrors = trackJsErrors(page);

    await createNotebook(page);

    // ── Cell 0: will become the shared cell ─────────────────────────────
    // "+ Markdown" also carries the base class — select "+ Code" explicitly.
    await page
      .locator(".ironpad-add-cell-btn", { hasText: "+ Code" })
      .first()
      .click();
    const sharedCell = page.locator(".ironpad-cell-card").first();
    await expect(sharedCell.locator(".monaco-editor").first()).toBeVisible({
      timeout: 15_000,
    });
    await setCellSource(
      page,
      sharedCell,
      "pub fn double(x: i32) -> i32 {\n    x * 2\n}"
    );

    // Source edits reach the model on a 1s debounce, and shared.rs is
    // assembled FROM THE MODEL at compile time — wait for the save indicator
    // (the tab's dirty dot) to clear before anything compiles against it.
    await expect(sharedCell.getByText("Code \u25cf")).toHaveCount(0, {
      timeout: 10_000,
    });

    // Mark it shared via the cell menu.
    await sharedCell.locator("..").locator('button[title="Cell menu"]').click();
    await page
      .locator(".ironpad-cell-menu-item", { hasText: "Mark as Shared" })
      .click();

    // Amber chrome + badge appear; the run button disappears.
    await expect(page.locator(".ironpad-cell-card--shared")).toHaveCount(1);
    await expect(
      page.locator(".ironpad-cell-type-badge--shared").first()
    ).toBeVisible();
    await expect(
      sharedCell.locator("..").locator('button[title="Run cell"]')
    ).toHaveCount(0);

    // ── Cell 1: consumes shared::double ─────────────────────────────────
    await page
      .locator(".ironpad-add-cell-btn", { hasText: "+ Code" })
      .last()
      .click();
    const consumer = page.locator(".ironpad-cell-card").nth(1);
    await expect(consumer.locator(".monaco-editor").first()).toBeVisible({
      timeout: 15_000,
    });
    await setCellSource(
      page,
      consumer,
      'CellOutput::text(format!("doubled={}", shared::double(21)))'
    );

    // Run All: only the consumer should execute.
    await page.locator(".ironpad-run-all-button").click();

    await expect(consumer.locator(".ironpad-cell-status--success")).toBeVisible(
      { timeout: 120_000 }
    );
    const outputText = consumer.locator(".ironpad-output-display-text");
    await expect(outputText).toBeVisible({ timeout: 5_000 });
    await expect(outputText).toContainText("doubled=42");

    // The shared cell never gained a success/compiling status badge.
    await expect(
      sharedCell.locator(".ironpad-cell-status--success")
    ).toHaveCount(0);

    expect(jsErrors).toEqual([]);
  });
});
