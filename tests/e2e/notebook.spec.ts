import { test, expect } from "@playwright/test";
import { setCellSource } from "./helpers/monaco";
import { trackJsErrors } from "./helpers/errors";
import { waitForPersistedCells } from "./helpers/session";

test.describe("Notebook", () => {
  test("create notebook and add cell", async ({ page }) => {
    // Collect JS errors during the test (filter known noise).
    const jsErrors = trackJsErrors(page);

    // Navigate to home page.
    await page.goto("/");
    await expect(page.locator(".ironpad-home")).toBeVisible();

    // Click "+ New Notebook" button.
    await page.waitForTimeout(3_000); // hydration (suite convention)
    await page.locator("button", { hasText: "+ New Notebook" }).click();

    // Verify navigation to /notebook/{id}.
    await expect(page).toHaveURL(/\/notebook\/[a-f0-9-]+/);

    // Verify the notebook editor is visible.
    await expect(page.locator(".ironpad-editor")).toBeVisible();

    // New notebooks start empty; verify the cell list is present.
    const cells = page.locator(".ironpad-cell-card");
    await expect(cells).toHaveCount(0);

    // Click "+ Add Cell" to add the first cell.
    await page.locator(".ironpad-add-cell-btn").first().click();

    // Verify a cell editor is now visible.
    await expect(cells).toHaveCount(1);
    await expect(page.locator(".ironpad-cell-editor-pane").first()).toBeVisible();

    // Verify the Monaco editor is mounted inside the cell (uat-003).
    await expect(
      cells.first().locator(".monaco-editor").first()
    ).toBeVisible({ timeout: 15_000 });

    // Click "+ Add Cell" again to add a second cell.
    await page.locator(".ironpad-add-cell-btn").last().click();

    // Verify two cells now exist.
    await expect(cells).toHaveCount(2);

    // Verify no JS errors occurred.
    expect(jsErrors).toEqual([]);
  });

  test("compile and execute a trivial cell", async ({ page }) => {
    // Compilation can take a while (cold cargo build to WASM).
    test.setTimeout(180_000);

    // Collect JS errors (filter known WASM hydration noise).
    const jsErrors = trackJsErrors(page);

    // Create a new notebook.
    await page.goto("/");
    await expect(page.locator(".ironpad-home")).toBeVisible();
    await page.waitForTimeout(3_000); // hydration (suite convention)
    await page.locator("button", { hasText: "+ New Notebook" }).click();
    await expect(page).toHaveURL(/\/notebook\/[a-f0-9-]+/);
    await expect(page.locator(".ironpad-editor")).toBeVisible();

    // Add a cell (default code: CellOutput::text("hello from ironpad").into()).
    await page.locator(".ironpad-add-cell-btn").first().click();
    const cell = page.locator(".ironpad-cell-card").first();
    await expect(cell).toBeVisible();

    // Click the run button ("▶") to compile and execute.
    // The run button lives in the side-actions panel alongside the card.
    const runButton = page.locator('button[title="Run cell"]').first();
    await expect(runButton).toBeVisible();
    await runButton.click();

    // Wait for compilation to start.
    await expect(cell.locator(".ironpad-cell-status--compiling")).toBeVisible({
      timeout: 5_000,
    });

    // Wait for compilation to finish (status leaves "compiling").
    await expect(cell.locator(".ironpad-cell-status--compiling")).toBeHidden({
      timeout: 120_000,
    });

    // Verify the cell reached success status.
    await expect(cell.locator(".ironpad-cell-status--success")).toBeVisible({
      timeout: 5_000,
    });

    // Verify the output panel appeared and contains the expected text.
    const outputText = cell.locator(".ironpad-output-display-text");
    await expect(outputText).toBeVisible({ timeout: 5_000 });
    await expect(outputText).toContainText("42");

    // Verify no JS errors occurred.
    expect(jsErrors).toEqual([]);
  });

  test("two-cell data flow via bincode", async ({ page }) => {
    // Two compilations back-to-back — generous timeout.
    test.setTimeout(300_000);

    const jsErrors = trackJsErrors(page);

    // ── Create a new notebook ───────────────────────────────────────────
    await page.goto("/");
    await expect(page.locator(".ironpad-home")).toBeVisible();
    await page.waitForTimeout(3_000); // hydration (suite convention)
    await page.locator("button", { hasText: "+ New Notebook" }).click();
    await expect(page).toHaveURL(/\/notebook\/[a-f0-9-]+/);
    await expect(page.locator(".ironpad-editor")).toBeVisible();

    // ── Add Cell 0 (producer) ───────────────────────────────────────────
    await page.locator("button", { hasText: "+ Code" }).first().click();
    const c0 = page.locator(".ironpad-cell-card").nth(0);
    await expect(c0).toBeVisible();
    await expect(c0.locator(".monaco-editor").first()).toBeVisible({
      timeout: 15_000,
    });

    // ── Add Cell 1 (consumer) ───────────────────────────────────────────
    await page.locator("button", { hasText: "+ Code" }).last().click();
    const c1 = page.locator(".ironpad-cell-card").nth(1);
    await expect(c1).toBeVisible();
    await expect(c1.locator(".monaco-editor").first()).toBeVisible({
      timeout: 15_000,
    });

    // ── Set cell sources via Monaco API ──────────────────────────────────
    // The scaffold wraps source in `({ source }).into()`, so the last
    // expression must be convertible to CellOutput.
    await setCellSource(
      page,
      c0,
      [
        'let data: Vec<i32> = vec![1, 2, 3, 4, 5];',
        'CellOutput::new(&data).unwrap().with_display(format!("Sent: {:?}", data))',
      ].join("\n")
    );

    await setCellSource(
      page,
      c1,
      [
        "let data: Vec<i32> = __ironpad_inputs__.get(0).deserialize().unwrap();",
        "let sum: i32 = data.iter().sum();",
        'CellOutput::text(format!("Sum: {}", sum))',
      ].join("\n")
    );

    // ── Run all cells via Ctrl+Shift+Enter ──────────────────────────────
    await page.keyboard.press("Control+Shift+Enter");

    // ── Wait for Cell 0 to compile and succeed ──────────────────────────
    await expect(c0.locator(".ironpad-cell-status--compiling")).toBeVisible({
      timeout: 10_000,
    });
    await expect(c0.locator(".ironpad-cell-status--compiling")).toBeHidden({
      timeout: 120_000,
    });
    await expect(c0.locator(".ironpad-cell-status--success")).toBeVisible({
      timeout: 10_000,
    });

    // Verify Cell 0 output shows the sent data.
    const cell0Output = c0.locator(".ironpad-output-display-text");
    await expect(cell0Output).toBeVisible({ timeout: 5_000 });
    await expect(cell0Output).toContainText("Sent: [1, 2, 3, 4, 5]");

    // ── Wait for Cell 1 to compile and succeed ──────────────────────────
    await expect(
      c1.locator(
        ".ironpad-cell-status--compiling, .ironpad-cell-status--running, .ironpad-cell-status--success"
      )
    ).toBeVisible({ timeout: 30_000 });
    await expect(
      c1.locator(".ironpad-cell-status--compiling")
    ).toBeHidden({ timeout: 120_000 });
    await expect(
      c1.locator(".ironpad-cell-status--running")
    ).toBeHidden({ timeout: 10_000 });
    await expect(c1.locator(".ironpad-cell-status--success")).toBeVisible({
      timeout: 10_000,
    });

    // ── Verify Cell 1 output shows the expected sum ─────────────────────
    const cell1Output = c1.locator(".ironpad-output-display-text");
    await expect(cell1Output).toBeVisible({ timeout: 5_000 });
    await expect(cell1Output).toContainText("Sum: 15");

    // Verify no JS errors occurred.
    expect(jsErrors).toEqual([]);
  });

  test("save and reload notebook preserves cell source", async ({ page }) => {
    // No compilation needed — just save/navigate; generous timeout for safety.
    test.setTimeout(60_000);

    // Collect JS errors (filter known WASM hydration noise).
    const jsErrors = trackJsErrors(page);

    // ── Create a new notebook ───────────────────────────────────────────
    await page.goto("/");
    await expect(page.locator(".ironpad-home")).toBeVisible();
    await page.waitForTimeout(3_000); // hydration (suite convention)
    await page.locator("button", { hasText: "+ New Notebook" }).click();
    await expect(page).toHaveURL(/\/notebook\/[a-f0-9-]+/);
    await expect(page.locator(".ironpad-editor")).toBeVisible();

    // ── Add a cell ──────────────────────────────────────────────────────
    await page.locator(".ironpad-add-cell-btn").first().click();
    const cell = page.locator(".ironpad-cell-card").first();
    await expect(cell).toBeVisible();

    // Wait for Monaco editor to mount and show content.
    await expect(cell.locator(".monaco-editor").first()).toBeVisible({
      timeout: 15_000,
    });
    const monacoContent = cell.locator(".monaco-editor .view-lines").first();
    await expect(monacoContent).toContainText("42", {
      timeout: 10_000,
    });

    // ── Save the notebook via Ctrl+S ────────────────────────────────────
    // Ctrl+S flushes editor content into the model and persists; poll the
    // durable store instead of sleeping (persistence is fire-and-forget).
    await page.keyboard.press("Control+s");
    await waitForPersistedCells(page, 1);

    // ── Navigate to home page ───────────────────────────────────────────
    await page.locator("a.ironpad-brand").click();
    await expect(page.locator(".ironpad-home")).toBeVisible();

    // ── Navigate back to the notebook ───────────────────────────────────
    // The notebook should appear in the list grid. Click on the first one.
    const notebookLink = page.locator(".ironpad-notebook-card-link").first();
    await expect(notebookLink).toBeVisible({ timeout: 5_000 });
    await notebookLink.click();

    // Verify we're back on the notebook editor page.
    await expect(page).toHaveURL(/\/notebook\/[a-f0-9-]+/);
    await expect(page.locator(".ironpad-editor")).toBeVisible();

    // ── Verify cell source is preserved ─────────────────────────────────
    const reloadedCell = page.locator(".ironpad-cell-card").first();
    await expect(reloadedCell).toBeVisible({ timeout: 10_000 });

    // Wait for Monaco editor to mount.
    await expect(
      reloadedCell.locator(".monaco-editor").first()
    ).toBeVisible({ timeout: 15_000 });

    // Verify the editor still contains the default cell source.
    const reloadedContent = reloadedCell.locator(
      ".monaco-editor .view-lines"
    ).first();
    await expect(reloadedContent).toContainText("42", {
      timeout: 10_000,
    });

    // Verify no JS errors occurred.
    expect(jsErrors).toEqual([]);
  });

  test("compiler errors render inline in Monaco with span highlighting", async ({
    page,
  }) => {
    test.setTimeout(180_000);

    const jsErrors = trackJsErrors(page);

    // ── Create a new notebook ───────────────────────────────────────────
    await page.goto("/");
    await expect(page.locator(".ironpad-home")).toBeVisible();
    await page.waitForTimeout(3_000); // hydration (suite convention)
    await page.locator("button", { hasText: "+ New Notebook" }).click();
    await expect(page).toHaveURL(/\/notebook\/[a-f0-9-]+/);
    await expect(page.locator(".ironpad-editor")).toBeVisible();

    // ── Add a cell ──────────────────────────────────────────────────────
    await page.locator(".ironpad-add-cell-btn").first().click();
    const cell = page.locator(".ironpad-cell-card").first();
    await expect(cell).toBeVisible();
    await expect(cell.locator(".monaco-editor").first()).toBeVisible({
      timeout: 15_000,
    });

    // ── Set broken Rust source via Monaco API ───────────────────────────
    await setCellSource(
      page,
      cell,
      [
        'let x: i32 = "not a number";',
        'CellOutput::text(format!("{}", x))',
      ].join("\n")
    );

    // ── Run the cell ────────────────────────────────────────────────────
    const runButton = page.locator('button[title="Run cell"]').first();
    await expect(runButton).toBeVisible();
    await runButton.click();

    // Wait for compilation to start.
    await expect(
      cell.locator(".ironpad-cell-status--compiling")
    ).toBeVisible({ timeout: 5_000 });

    // Wait for compilation to finish.
    await expect(
      cell.locator(".ironpad-cell-status--compiling")
    ).toBeHidden({ timeout: 120_000 });

    // ── Verify error state ──────────────────────────────────────────────
    await expect(
      cell.locator(".ironpad-cell-status--error")
    ).toBeVisible({ timeout: 5_000 });

    // ── Verify Monaco inline error markers (squiggly underlines) ────────
    // Monaco renders error markers with the `.squiggly-error` CSS class.
    await expect(
      cell.locator(".monaco-editor .squiggly-error")
    ).toBeVisible({ timeout: 10_000 });

    // ── Verify error panel with diagnostics ─────────────────────────────
    const errorPanel = cell.locator(".ironpad-error-panel");
    await expect(errorPanel).toBeVisible({ timeout: 5_000 });

    // The error panel should mention "mismatched types" (rustc E0308).
    await expect(errorPanel).toContainText(/mismatched types/i, {
      timeout: 5_000,
    });

    // Verify no JS errors occurred.
    expect(jsErrors).toEqual([]);
  });
});
