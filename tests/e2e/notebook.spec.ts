import { test, expect, Page } from "@playwright/test";
import { menuClick } from "./helpers/menu";
import { setCellSource } from "./helpers/monaco";
import { trackJsErrors } from "./helpers/errors";
import {
  createNotebook,
  waitForPersistedCells,
  ADD_CODE,
} from "./helpers/session";

/** The open local notebook's id, from the URL. */
function localId(page: Page): string {
  return page.url().match(/\/local\/([a-f0-9-]+)/)![1];
}

/** The stored record for a local notebook, or null when there is none. */
function storedNotebook(page: Page, id: string): Promise<any> {
  return page.evaluate(
    (nid) => (window as any).IronpadStorage.getNotebook(nid),
    id,
  );
}

test.describe("Notebook", () => {
  test("create notebook and add cell", async ({ page }) => {
    // Collect JS errors during the test (filter known noise).
    const jsErrors = trackJsErrors(page);

    // Navigate to home page.
    await page.goto("/");
    await expect(page.locator(".ironpad-home")).toBeVisible();

    // Click "New Notebook" button.
    await page.waitForTimeout(3_000); // hydration (suite convention)
    await page.locator("button", { hasText: "New Notebook" }).click();

    // Verify navigation to /notebook/{id}.
    await expect(page).toHaveURL(/\/local\/[a-f0-9-]+/);

    // Verify the notebook editor is visible.
    await expect(page.locator(".ironpad-editor")).toBeVisible();

    // New notebooks start empty; verify the cell list is present.
    const cells = page.locator(".ironpad-cell-card");
    await expect(cells).toHaveCount(0);

    // Click "Add Cell" to add the first cell.
    await page.locator(".ironpad-add-cell-btn").first().click();

    // Verify a cell editor is now visible.
    await expect(cells).toHaveCount(1);
    await expect(page.locator(".ironpad-cell-editor-pane").first()).toBeVisible();

    // Verify the Monaco editor is mounted inside the cell (uat-003).
    await expect(
      cells.first().locator(".monaco-editor").first()
    ).toBeVisible({ timeout: 15_000 });

    // Click "Add Cell" again to add a second cell. Named explicitly rather
    // than `.last()`: PRD-0066 put a third button (Linux) in every add row,
    // so the last one stopped being Markdown and started being a cell with a
    // different execution model entirely.
    await page.locator(ADD_CODE).last().click();

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
    await page.locator("button", { hasText: "New Notebook" }).click();
    await expect(page).toHaveURL(/\/local\/[a-f0-9-]+/);
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
    // Wait for compilation to finish. Only the terminal state is
    // asserted: "compiling" is transient and a warm cache can skip
    // past it between polls.
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
    await page.locator("button", { hasText: "New Notebook" }).click();
    await expect(page).toHaveURL(/\/local\/[a-f0-9-]+/);
    await expect(page.locator(".ironpad-editor")).toBeVisible();

    // ── Add Cell 0 (producer) ───────────────────────────────────────────
    await page.locator(ADD_CODE).first().click();
    const c0 = page.locator(".ironpad-cell-card").nth(0);
    await expect(c0).toBeVisible();
    await expect(c0.locator(".monaco-editor").first()).toBeVisible({
      timeout: 15_000,
    });

    // ── Add Cell 1 (consumer) ───────────────────────────────────────────
    await page.locator(ADD_CODE).last().click();
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

    // ── Wait for Cell 0 to succeed ──────────────────────────────────────
    //
    // Assert the TERMINAL state only: "compiling" is transient, and on a
    // warm cache it can come and go between polls (this assertion failed
    // exactly once, right after a run that warmed these cells' blobs).
    await expect(c0.locator(".ironpad-cell-status--success")).toBeVisible({
      timeout: 120_000,
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
    // No compilation needed — just save/reload; generous timeout for safety.
    test.setTimeout(60_000);

    // Collect JS errors (filter known WASM hydration noise).
    const jsErrors = trackJsErrors(page);

    // ── Create a new notebook ───────────────────────────────────────────
    await page.goto("/");
    await expect(page.locator(".ironpad-home")).toBeVisible();
    await page.waitForTimeout(3_000); // hydration (suite convention)
    await page.locator("button", { hasText: "New Notebook" }).click();
    await expect(page).toHaveURL(/\/local\/[a-f0-9-]+/);
    await expect(page.locator(".ironpad-editor")).toBeVisible();

    // ── Add a cell ──────────────────────────────────────────────────────
    await page.locator(".ironpad-add-cell-btn").first().click();
    const cell = page.locator(".ironpad-cell-card").first();
    await expect(cell).toBeVisible();

    // Wait for Monaco editor to mount.
    await expect(cell.locator(".monaco-editor").first()).toBeVisible({
      timeout: 15_000,
    });

    // ── Type a source that only this test could have produced ──────────
    // Every new Code cell is seeded with `42`, so asserting that would hold
    // even if nothing typed were ever saved.
    const marker = "saved_marker_9137";
    await setCellSource(page, cell, `let ${marker} = 1;`);

    // ── Save the notebook via Ctrl+S ────────────────────────────────────
    // Ctrl+S flushes editor content into the model and persists; poll the
    // durable store for the typed source instead of sleeping (persistence is
    // fire-and-forget).
    await page.keyboard.press("Control+s");
    const notebookId = page.url().match(/\/local\/([a-f0-9-]+)/)![1];
    await expect
      .poll(
        () =>
          page.evaluate(
            async (id) =>
              (await (window as any).IronpadStorage.getNotebook(id))?.cells?.[0]
                ?.source ?? "",
            notebookId
          ),
        { timeout: 10_000 }
      )
      .toContain(marker);

    // ── Reload the page ─────────────────────────────────────────────────
    // A real reload, so the source has to come back out of IndexedDB rather
    // than out of the still-running wasm instance's model.
    await page.reload();
    await expect(page.locator(".ironpad-editor")).toBeVisible({
      timeout: 15_000,
    });

    // ── Verify cell source is preserved ─────────────────────────────────
    const reloadedCell = page.locator(".ironpad-cell-card").first();
    await expect(reloadedCell.locator(".monaco-editor").first()).toBeVisible({
      timeout: 15_000,
    });
    await expect(
      reloadedCell.locator(".monaco-editor .view-lines").first()
    ).toContainText(marker, { timeout: 10_000 });

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
    await page.locator("button", { hasText: "New Notebook" }).click();
    await expect(page).toHaveURL(/\/local\/[a-f0-9-]+/);
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

test.describe("Leaving the editor", () => {
  /** A fresh notebook with one persisted code cell whose editor is mounted. */
  async function notebookWithCell(page: Page) {
    await createNotebook(page);
    await page.locator(ADD_CODE).first().click();
    const cell = page.locator(".ironpad-cell-card").first();
    await expect(cell.locator(".monaco-editor").first()).toBeVisible({
      timeout: 15_000,
    });
    await waitForPersistedCells(page, 1);
    return cell;
  }

  test("closing right after typing keeps the edit", async ({ page }) => {
    // The 1s save debounce used to die with the page: in-app navigation
    // cancelled the timer and nothing flushed, so the store kept `42`.
    const cell = await notebookWithCell(page);
    const id = localId(page);
    await setCellSource(page, cell, "42 + 777");
    await page.locator(".ironpad-toolbar-close").click();
    await expect(page.locator(".ironpad-home")).toBeVisible({ timeout: 15_000 });
    await expect
      .poll(async () => (await storedNotebook(page, id))?.cells?.[0]?.source, {
        timeout: 10_000,
      })
      .toBe("42 + 777");
  });

  test("opening and closing an untouched notebook writes nothing", async ({
    page,
  }) => {
    // The negative half of the teardown write: it fires only for edits no
    // write has landed. Unconditional, it would bump `updated_at` and mint
    // a history snapshot on every visit.
    await notebookWithCell(page);
    const id = localId(page);
    await page.reload();
    await expect(
      page.locator(".ironpad-cell-card .monaco-editor").first(),
    ).toBeVisible({ timeout: 15_000 });
    await page.waitForTimeout(1_500);
    const before = (await storedNotebook(page, id)).updated_at;
    await page.locator(".ironpad-toolbar-close").click();
    await expect(page.locator(".ironpad-home")).toBeVisible({ timeout: 15_000 });
    await page.waitForTimeout(2_000);
    expect((await storedNotebook(page, id)).updated_at).toBe(before);
  });

  test("a save debounce firing behind Delete's confirm cannot bring the notebook back", async ({
    page,
  }) => {
    // The cell's 1s debounce comes due while the blocking confirm is up and
    // fires the moment it returns, right beside the delete. Its write used
    // to land second and resurrect the record.
    const cell = await notebookWithCell(page);
    const id = localId(page);
    await setCellSource(page, cell, "42 + 1");
    page.once("dialog", async (dialog) => {
      await new Promise((resolve) => setTimeout(resolve, 1_300));
      await dialog.accept();
    });
    await menuClick(page, "Delete");
    await expect(page.locator(".ironpad-home")).toBeVisible({ timeout: 15_000 });
    await page.waitForTimeout(2_000);
    expect(await storedNotebook(page, id)).toBeNull();
  });
});

test.describe("Save results", () => {
  test("a browser that refuses the write is reported, once per failure streak", async ({
    page,
  }) => {
    await createNotebook(page);
    await page.locator(ADD_CODE).first().click();
    const cell = page.locator(".ironpad-cell-card").first();
    await expect(cell.locator(".monaco-editor").first()).toBeVisible({
      timeout: 15_000,
    });
    await waitForPersistedCells(page, 1);
    const notSaved = page
      .locator(".ironpad-toast-title")
      .filter({ hasText: /^Not Saved$/ });

    // The store starts refusing writes, the way a full or private-mode
    // browser does. The storage binding resolves the global at call time.
    await page.evaluate(() => {
      const storage = (window as any).IronpadStorage;
      storage.__realSave = storage.saveNotebook;
      storage.saveNotebook = async () => {
        throw new DOMException("refused by the test", "QuotaExceededError");
      };
    });
    // Two autosaves, one toast.
    await setCellSource(page, cell, "42 + 1");
    await expect(notSaved).toHaveCount(1, { timeout: 10_000 });
    await setCellSource(page, cell, "42 + 2");
    await page.waitForTimeout(2_000);
    await expect(notSaved).toHaveCount(1);
    // An explicit save always reports, and never claims success.
    await cell.locator(".ironpad-cell-header").click();
    await page.keyboard.press("Control+s");
    await expect(notSaved).toHaveCount(2, { timeout: 10_000 });
    await expect(
      page.locator(".ironpad-toast-title", { hasText: "Notebook saved" }),
    ).toHaveCount(0);

    await page.evaluate(() => {
      const storage = (window as any).IronpadStorage;
      storage.saveNotebook = storage.__realSave;
    });
    await page.keyboard.press("Control+s");
    await expect(
      page.locator(".ironpad-toast-title", { hasText: "Notebook saved" }),
    ).toBeVisible({ timeout: 10_000 });
  });
});

test.describe("A notebook that does not load", () => {
  // The Local load folded "not found", "read failed" and "not a notebook"
  // into one None with no else, so the skeleton animated forever.
  test("a notebook not in this browser says so", async ({ page }) => {
    await page.goto("/local/00000000-0000-4000-8000-000000000000");
    await expect(
      page.locator(".ironpad-error-boundary-message", {
        hasText: "This notebook was not found.",
      }),
    ).toBeVisible({ timeout: 15_000 });
    await expect(page.locator(".ironpad-error-boundary-hint")).toContainText(
      "Local notebooks live only in the browser that created them",
    );
    await expect(page.locator(".ironpad-cell-skeleton")).toHaveCount(0);
  });

  test("a record that is not a notebook says it could not be read", async ({
    page,
  }) => {
    await page.goto("/");
    await page.waitForFunction(() => (window as any).IronpadStorage, null, {
      timeout: 15_000,
    });
    const uuid = "11111111-1111-4111-8111-111111111111";
    // No cells: the record exists but does not parse as a notebook.
    await page.evaluate(
      (id) => (window as any).IronpadStorage.saveNotebook({ id, title: "x" }),
      uuid,
    );
    await page.goto(`/local/${uuid}`);
    await expect(
      page.locator(".ironpad-error-boundary-message", {
        hasText: "This notebook could not be read.",
      }),
    ).toBeVisible({ timeout: 15_000 });
    await expect(page.locator(".ironpad-cell-skeleton")).toHaveCount(0);
  });
});
