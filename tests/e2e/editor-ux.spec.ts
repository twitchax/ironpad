import { test, expect } from "@playwright/test";
import { MENU, menuClick } from "./helpers/menu";
import { setCellSource } from "./helpers/monaco";
import { createNotebook as newNotebook } from "./helpers/session";

/**
 * End-to-end coverage for the PRD-0032 editor UX fixes.
 *
 *   uat-001 — a structural edit (adding a cell) preserves other cells' output
 *   uat-002 — renaming a notebook persists across reload
 *   uat-003 — switching to view mode renders an in-edit markdown cell
 *   uat-005 — Share surfaces the share URL
 *   uat-004 — cancel does not re-run on the main thread (see note below)
 *
 * Notebook creation goes through the hardened shared helper: a local copy
 * without the hydration wait raced the WASM click wiring under full-suite
 * parallel load (the "+ New Notebook" click landed on a dead button).
 */

test.describe("Editor UX (PRD-0032)", () => {
  // uat-002: renaming a notebook persists across reload.
  test("uat-002: notebook title rename persists across reload", async ({
    page,
  }) => {
    await newNotebook(page);
    const url = page.url();

    // Click the title to enter edit mode, replace it, commit with Enter.
    await page.locator(".ironpad-notebook-title").click();
    const input = page.locator(".ironpad-header-title-input");
    await expect(input).toBeVisible();
    await input.fill("Persisted Rename");
    await input.press("Enter");

    // Wait for the commit to reflect in the header before reloading —
    // navigating immediately races the async IndexedDB write.
    await expect(page.locator(".ironpad-header-center")).toContainText(
      "Persisted Rename"
    );

    // Reload from IndexedDB and verify the new title survived.
    await page.goto(url);
    await expect(page.locator(".ironpad-header-center")).toContainText(
      "Persisted Rename",
      { timeout: 15_000 }
    );
  });

  // Typing into the title must accumulate characters. The edit input used to
  // remount on every keystroke (the branch closure tracked the title's
  // content), and the remount's focus effect select()ed the text — so each
  // key replaced the whole field with one character. fill() sets the value in
  // one shot and masked this; type character by character instead.
  test("title input accumulates keystrokes", async ({ page }) => {
    await newNotebook(page);

    await page.locator(".ironpad-notebook-title").click();
    const input = page.locator(".ironpad-header-title-input");
    await expect(input).toBeVisible();

    // Clear first: entering edit mode select()s the existing title on a
    // queued effect, and racing that makes the final value depend on timing.
    // With the field empty, per-keystroke accumulation is the whole signal —
    // the pre-fix remount+select left only the LAST typed character here.
    await input.fill("");
    await input.pressSequentially("My Great Notebook", { delay: 20 });
    await expect(input).toHaveValue("My Great Notebook");
  });

  // uat-003: switching to view mode renders an in-edit markdown cell
  // (previously it stayed a raw Monaco editor).
  test("uat-003: view mode forces an in-edit markdown cell to render", async ({
    page,
  }) => {
    await newNotebook(page);

    // Add a markdown cell and enter edit mode by double-clicking the preview.
    await page.locator("button", { hasText: "+ Markdown" }).first().click();
    const cell = page.locator(".ironpad-cell-card").first();
    await expect(cell).toBeVisible();
    await cell.locator(".ironpad-markdown-cell-preview").dblclick();
    await expect(cell.locator(".ironpad-markdown-cell-editor")).toBeVisible();

    // Switch to view mode — the public renderer takes over: the in-edit
    // markdown commits and renders as a view-only cell, no Monaco editor.
    await page.locator('button[title="View mode"]').click();
    await expect(page.locator(".view-only-markdown")).toBeVisible({
      timeout: 10_000,
    });
    await expect(page.locator(".ironpad-markdown-cell-editor")).toHaveCount(0);
  });

  // The notebook-level actions (hamburger menu, close) survive the flip to
  // view mode; editing-only chrome (Run All, session, gear) does not — view
  // mode's own header carries Run All and the cache/fresh toggle.
  test("notebook menu chrome is available in view mode", async ({ page }) => {
    await newNotebook(page);

    await page.locator('button[title="View mode"]').click();
    await expect(page.locator(".view-only-notebook")).toBeVisible({
      timeout: 10_000,
    });

    // Available: hamburger + close. Hidden: editing-only toolbar chrome.
    await expect(page.locator(MENU)).toBeVisible();
    await expect(
      page.locator('button[title="Back to notebook list"]')
    ).toBeVisible();
    await expect(page.locator('button[title="Notebook settings"]')).toHaveCount(
      0
    );
    await expect(page.locator(".ironpad-run-all-button")).toHaveCount(0);

    // The menu's actions work from view mode: Share surfaces the share URL.
    await menuClick(page, "Share Immutable");
    await expect(page.getByText(/\/shared\//).first()).toBeVisible({
      timeout: 30_000,
    });
  });

  // A dropdown item that toggles state must not close the menu under itself.
  // Regression (PRD-0062): menu items render icon markup now, so the click
  // target is a span the toggle's re-render detaches mid-dispatch. The
  // outside-click handler read `target.closest(...)` on that detached node,
  // saw no ancestor, and closed the menu — which silently inverted the
  // open/closed parity for every subsequent toggle.
  test("toggling a gear-menu item keeps the menu open", async ({ page }) => {
    await newNotebook(page);
    const gear = page.locator('button[title="Notebook settings"]');
    const item = page.locator(".ironpad-toolbar-dropdown-item", {
      hasText: "Force Recompile",
    });
    const menu = page.locator(".ironpad-toolbar-dropdown-menu");

    await gear.click();
    await expect(menu).toBeVisible();
    await item.first().click();
    await expect(menu).toBeVisible();

    // ...and a genuine click outside still closes it.
    await page.locator(".ironpad-editor").click({ position: { x: 5, y: 5 } });
    await expect(menu).toHaveCount(0);
  });

  // Rendered markdown fenced code blocks are syntax-highlighted by Prism
  // (public/prism/highlight-code.js), not just Monaco-highlighted while editing.
  test("rendered code block is syntax-highlighted by Prism", async ({
    page,
  }) => {
    await newNotebook(page);

    // Add a markdown cell, edit it, and give it a fenced Rust block.
    await page.locator("button", { hasText: "+ Markdown" }).first().click();
    const cell = page.locator(".ironpad-cell-card").first();
    await expect(cell).toBeVisible();
    await cell.locator(".ironpad-markdown-cell-preview").dblclick();
    await expect(cell.locator(".ironpad-markdown-cell-editor")).toBeVisible();
    await expect(cell.locator(".monaco-editor").first()).toBeVisible({
      timeout: 15_000,
    });
    await setCellSource(page, cell, "```rust\nlet x: u32 = 42;\n```");

    // Render it (view mode commits + renders via the public renderer).
    await page.locator('button[title="View mode"]').click();

    // The fence keeps its language class...
    const code = page.locator(".view-only-markdown code.language-rust");
    await expect(code).toBeVisible({ timeout: 10_000 });

    // ...and Prism has rewritten it into token spans — the `let` keyword is the
    // regression anchor. Monaco's editor highlighting is a separate DOM; this
    // asserts the RENDERED output is highlighted.
    await expect(code.locator(".token.keyword", { hasText: "let" })).toHaveCount(
      1,
      { timeout: 10_000 }
    );
  });

  // Per-cell collapse defaults: the header toggle collapses the code live,
  // persists on the cell (IndexedDB), and every mode loads the cell in that
  // state. The chevron stays a transient affordance on top.
  test("cell collapse toggle persists and drives edit and view modes", async ({
    page,
  }) => {
    await newNotebook(page);
    const url = page.url();

    // Add a code cell — open by default.
    await page.locator("button", { hasText: "+ Code" }).first().click();
    const cell = page.locator(".ironpad-cell-card").first();
    await expect(cell.locator(".monaco-editor").first()).toBeVisible({
      timeout: 15_000,
    });
    const body = cell.locator(".ironpad-cell-body");
    await expect(body).not.toHaveClass(/ironpad-cell-body--collapsed/);

    // The code toggle collapses the body live...
    await cell.locator(".ironpad-collapse-default-btn").first().click();
    await expect(body).toHaveClass(/ironpad-cell-body--collapsed/);

    // ...and persists (poll the durable store; persist_notebook is
    // fire-and-forget, so a straight reload races the write).
    const notebookId = url.match(/\/local\/([a-f0-9-]+)/)![1];
    await expect
      .poll(
        () =>
          page.evaluate(async (id) => {
            const nb = await (window as any).IronpadStorage.getNotebook(id);
            return nb?.cells?.[0]?.collapsed ?? false;
          }, notebookId),
        { timeout: 10_000 }
      )
      .toBe(true);

    // Reload: the cell loads collapsed in edit mode.
    await page.goto(url);
    const reloadedCell = page.locator(".ironpad-cell-card").first();
    await expect(reloadedCell).toBeVisible({ timeout: 15_000 });
    const reloadedBody = reloadedCell.locator(".ironpad-cell-body");
    await expect(reloadedBody).toHaveClass(/ironpad-cell-body--collapsed/);

    // View mode swaps in the public renderer and loads the cell collapsed
    // there too; the chevron still opens it transiently, and its editor is
    // read-only (all view-only editors are).
    await page.locator('button[title="View mode"]').click();
    const viewCell = page.locator(".view-only-cell").first();
    await expect(viewCell).toBeVisible({ timeout: 10_000 });
    const viewBody = viewCell.locator(".ironpad-cell-body");
    await expect(viewBody).toHaveClass(/ironpad-cell-body--collapsed/);
    await viewCell.locator(".ironpad-cell-collapse-btn").click();
    await expect(viewBody).not.toHaveClass(/ironpad-cell-body--collapsed/);
    await expect(viewCell.locator(".monaco-editor").first()).toBeVisible({
      timeout: 15_000,
    });
    const readOnly = await page.evaluate(() => {
      const monaco = (window as any).monaco;
      const el = document.querySelector(".view-only-cell");
      const editor = monaco.editor
        .getEditors()
        .find((e: any) => el!.contains(e.getDomNode()));
      return editor.getOption(monaco.editor.EditorOption.readOnly);
    });
    expect(readOnly).toBe(true);

    // The authoring toggles are edit-mode chrome — absent in the public
    // renderer.
    await expect(page.locator(".ironpad-collapse-defaults")).toHaveCount(0);

    // Back to edit mode: the rebuilt cell rows re-read the model's cell
    // manifests, so the header toggle must still show collapsed-by-default
    // (regression: collapse-only updates once skipped the manifest sync,
    // so the round trip silently reset the toggle).
    await page.locator('button[title="Edit mode"]').click();
    const editCell = page.locator(".ironpad-cell-card").first();
    await expect(editCell).toBeVisible({ timeout: 10_000 });
    await expect(
      editCell.locator(".ironpad-collapse-default-btn").first()
    ).toHaveClass(/ironpad-collapse-default-btn--collapsed/);
    await expect(editCell.locator(".ironpad-cell-body")).toHaveClass(
      /ironpad-cell-body--collapsed/
    );
  });

  // The output toggle: collapses the output panel live and persists the flag.
  test("output collapse toggle collapses the panel and persists", async ({
    page,
  }) => {
    test.setTimeout(300_000);
    await newNotebook(page);
    const url = page.url();

    // Run a trivial cell so the output panel exists ("42" is warm from the
    // other specs' compiles).
    await page.locator("button", { hasText: "+ Code" }).first().click();
    const cell = page.locator(".ironpad-cell-card").first();
    await expect(cell.locator(".monaco-editor").first()).toBeVisible({
      timeout: 15_000,
    });
    await page.locator('button[title="Run cell"]').first().click();
    await expect(cell.locator(".ironpad-output-panel")).toBeVisible({
      timeout: 240_000,
    });
    await expect(cell.locator(".ironpad-output-panel")).not.toHaveClass(
      /--collapsed/
    );

    // The output toggle (second button in the pill) collapses it live...
    await cell.locator(".ironpad-collapse-default-btn").nth(1).click();
    await expect(cell.locator(".ironpad-output-panel")).toHaveClass(
      /ironpad-output-panel--collapsed/
    );

    // ...and the flag lands in the durable store.
    const notebookId = url.match(/\/local\/([a-f0-9-]+)/)![1];
    await expect
      .poll(
        () =>
          page.evaluate(async (id) => {
            const nb = await (window as any).IronpadStorage.getNotebook(id);
            return nb?.cells?.[0]?.output_collapsed ?? false;
          }, notebookId),
        { timeout: 10_000 }
      )
      .toBe(true);
  });

  // uat-001: adding a cell preserves an already-run cell's output.
  // Requires a real compile → generous timeout.
  test("uat-001: adding a cell preserves other cells' output", async ({
    page,
  }) => {
    test.setTimeout(300_000);
    await newNotebook(page);

    // Add a code cell, set trivial source, run it.
    await page.locator("button", { hasText: "+ Code" }).first().click();
    const cells = page.locator(".ironpad-cell-card");
    await expect(cells).toHaveCount(1);
    await expect(cells.first().locator(".monaco-editor").first()).toBeVisible({
      timeout: 15_000,
    });
    await setCellSource(page, cells.first(), "42");
    await page.locator('button[title="Run cell"]').first().click();

    // Wait for success + output.
    await expect(cells.first().locator(".ironpad-cell-status--success")).toBeVisible(
      { timeout: 240_000 }
    );
    const firstOutput = cells.first().locator(".ironpad-output-display-text");
    await expect(firstOutput).toContainText("42");

    // Add a second cell — the first cell's output must NOT be wiped (the E1 bug).
    await page.locator("button", { hasText: "+ Code" }).last().click();
    await expect(cells).toHaveCount(2);
    await expect(cells.first().locator(".ironpad-cell-status--success")).toBeVisible();
    await expect(cells.first().locator(".ironpad-output-display-text")).toContainText(
      "42"
    );
  });

  // uat-005: Share surfaces the share URL (toast with a /shared/ link).
  test("uat-005: Share surfaces the share URL", async ({ page, context }) => {
    // Grant clipboard so the Share handler's clipboard write doesn't reject.
    await context.grantPermissions(["clipboard-read", "clipboard-write"]);
    await newNotebook(page);
    await page.locator("button", { hasText: "+ Code" }).first().click();
    await expect(page.locator(".ironpad-cell-card")).toHaveCount(1);

    // Open the hamburger menu and click Share.
    await menuClick(page, "Share Immutable");

    // A success toast containing the shared URL should appear.
    await expect(page.getByText(/\/shared\//).first()).toBeVisible({
      timeout: 30_000,
    });
  });

  // uat-004: cancelling a running cell must NOT re-run it on the main thread.
  // This is guarded at the source (executor-bridge.js rethrows AbortError instead
  // of falling back). Automating it reliably requires a long-running cell and
  // asserting the tab stays responsive, which is flaky in CI; the behavior is
  // covered by the code-level guard and manual verification. Left as a documented
  // placeholder so the coverage gap is explicit.
  test.skip("uat-004: cancel does not re-run on the main thread", async () => {
    // Intentionally skipped — see comment above.
  });
});
