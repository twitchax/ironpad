import { test, expect } from "@playwright/test";
import { setCellSource } from "./helpers/monaco";
import { ADD_CODE, createNotebook } from "./helpers/session";

/**
 * A widget inside a tuple output must not overwrite the whole tuple's piped
 * bytes when it moves. The tuple `(slider, slider)` pipes 16 bytes under a
 * `(f64, f64)` tag; a slider change once replaced them with ONE f64's 8 bytes,
 * and the next run of a downstream cell reading `cell0` panicked decoding
 * them. A tuple-embedded widget now publishes through the sim bus only, so the
 * downstream cell keeps reading the tuple's defaults.
 */
test("moving a slider inside a tuple output keeps downstream cells decodable", async ({
  page,
}) => {
  test.setTimeout(300_000);
  await createNotebook(page);

  const sources = [
    '(ui::slider("a", 0.0, 10.0), ui::slider("b", 0.0, 10.0))',
    "let (a, b) = cell0; a + b",
  ];
  for (let i = 0; i < sources.length; i += 1) {
    await page.locator(ADD_CODE).last().click();
    const cell = page.locator(".ironpad-cell-card").nth(i);
    await expect(cell.locator(".monaco-editor").first()).toBeVisible({ timeout: 15_000 });
    await setCellSource(page, cell, sources[i]);
  }
  const cells = page.locator(".ironpad-cell-card");

  // Running the consumer cascades the slider cell first.
  await page.locator('button[title="Run cell"]').nth(1).click();
  await expect(cells.nth(1).locator(".ironpad-cell-status--success")).toBeVisible({
    timeout: 240_000,
  });

  // Drag the first slider (programmatically: Playwright cannot fill a range).
  const slider = cells.nth(0).locator('input[type="range"]').first();
  await expect(slider).toBeVisible();
  await slider.evaluate((el) => {
    (el as HTMLInputElement).value = "5";
    el.dispatchEvent(new Event("input", { bubbles: true }));
  });

  // Re-run the consumer with an edit, so its fresh output is distinguishable
  // from the first run's.
  await setCellSource(page, cells.nth(1), "let (a, b) = cell0; a + b + 100.0");
  await page.locator('button[title="Run cell"]').nth(1).click();
  await expect(cells.nth(1).locator(".ironpad-output-display-text")).toContainText("100", {
    timeout: 240_000,
  });
  await expect(cells.nth(1).locator(".ironpad-cell-status--success")).toBeVisible();
  await expect(cells.nth(1)).not.toContainText("failed to deserialize");
});
