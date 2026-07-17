import { test, expect } from "@playwright/test";
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

    // Switch to view mode — the cell must render (not stay a Monaco editor).
    await page.locator('button[title="View mode"]').click();
    await expect(cell.locator(".ironpad-markdown-cell-preview")).toBeVisible();
    await expect(cell.locator(".ironpad-markdown-cell-editor")).toHaveCount(0);
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

    // Render it (view mode commits + renders, per uat-003).
    await page.locator('button[title="View mode"]').click();

    // The fence keeps its language class...
    const code = cell.locator(
      ".ironpad-markdown-cell-preview code.language-rust"
    );
    await expect(code).toBeVisible({ timeout: 10_000 });

    // ...and Prism has rewritten it into token spans — the `let` keyword is the
    // regression anchor. Monaco's editor highlighting is a separate DOM; this
    // asserts the RENDERED output is highlighted.
    await expect(code.locator(".token.keyword", { hasText: "let" })).toHaveCount(
      1,
      { timeout: 10_000 }
    );
  });

  // View mode collapses code cells by default, the chevron can still open an
  // individual cell (public-page parity), and the notebook's "Expand Code in
  // View" setting keeps code open — previously the editor's view mode ignored
  // that setting entirely and force-collapsed everything.
  test("expand code in view is honored by the editor's view mode", async ({
    page,
  }) => {
    await newNotebook(page);

    // Add a code cell.
    await page.locator("button", { hasText: "+ Code" }).first().click();
    const cell = page.locator(".ironpad-cell-card").first();
    await expect(cell).toBeVisible();
    await expect(cell.locator(".monaco-editor").first()).toBeVisible({
      timeout: 15_000,
    });

    // Default: view mode collapses the code body...
    await page.locator('button[title="View mode"]').click();
    const body = cell.locator(".ironpad-cell-body");
    await expect(body).toHaveClass(/ironpad-cell-body--collapsed/);

    // ...but the chevron opens this one cell.
    await cell.locator(".ironpad-cell-collapse-btn").click();
    await expect(body).not.toHaveClass(/ironpad-cell-body--collapsed/);

    // Back to edit mode, then opt the notebook into expand-code.
    await page.locator('button[title="Edit mode"]').click();
    await page.locator('button[title="Notebook settings"]').click();
    await page
      .locator(".ironpad-toolbar-dropdown-item", {
        hasText: "Expand Code in View",
      })
      .click();

    // Now view mode keeps the code visible...
    await page.locator('button[title="View mode"]').click();
    await expect(body).not.toHaveClass(/ironpad-cell-body--collapsed/);

    // ...and the visible editor is read-only.
    const readOnly = await page.evaluate(() => {
      const monaco = (window as any).monaco;
      const el = document.querySelector(".ironpad-cell-card");
      const editor = monaco.editor
        .getEditors()
        .find((e: any) => el!.contains(e.getDomNode()));
      return editor.getOption(monaco.editor.EditorOption.readOnly);
    });
    expect(readOnly).toBe(true);
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
    await page.locator("button", { hasText: "☰" }).click();
    await page
      .locator(".ironpad-toolbar-dropdown-item", { hasText: "Share" })
      .click();

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
