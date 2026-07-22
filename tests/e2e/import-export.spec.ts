import { test, expect } from "@playwright/test";
import * as fs from "fs";
import * as path from "path";
import { trackJsErrors } from "./helpers/errors";

// Extend Window to include IronpadStorage (defined in public/storage.js).
declare global {
  interface Window {
    IronpadStorage: {
      listNotebooks: () => Promise<Array<{ id: string; title: string }>>;
      importNotebook: (json: string) => Promise<{ id: string; title: string }>;
    };
  }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/** A minimal valid .ironpad notebook fixture for testing. */
function makeFixture(title: string) {
  return {
    version: 1,
    id: "00000000-0000-0000-0000-e2eimporttest",
    title,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    shared_cargo_toml: "",
    cells: [
      {
        id: "e2e_cell_0",
        order: 0,
        label: "Test Cell",
        cell_type: "Code",
        source: 'CellOutput::text("imported").into()\n',
        cargo_toml: "",
      },
    ],
  };
}

// ── Tests ────────────────────────────────────────────────────────────────────

test.describe("Notebook import/export", () => {
  test("download notebook as .ironpad file", async ({ page }) => {
    // Allow time for notebook creation and download.
    test.setTimeout(60_000);

    const jsErrors = trackJsErrors(page);

    // ── Create a new notebook with a cell ────────────────────────────────
    await page.goto("/");
    await expect(page.locator(".ironpad-home")).toBeVisible();
    await page.waitForTimeout(3_000); // hydration (suite convention)
    await page.locator("button", { hasText: "+ New Notebook" }).click();
    await expect(page).toHaveURL(/\/local\/[a-f0-9-]+/);
    await expect(page.locator(".ironpad-editor")).toBeVisible();

    // Add a cell so the notebook has content.
    await page.locator(".ironpad-add-cell-btn").first().click();
    await expect(page.locator(".ironpad-cell-card")).toHaveCount(1);

    // Wait for Monaco editor to mount and autosave to settle.
    await expect(
      page
        .locator(".ironpad-cell-card")
        .first()
        .locator(".monaco-editor")
        .first()
    ).toBeVisible({ timeout: 15_000 });

    // Allow time for the notebook to persist to IndexedDB.
    await page.waitForTimeout(3_000);

    // ── Open hamburger menu and download ─────────────────────────────────
    const hamburgerBtn = page
      .locator(".ironpad-toolbar-dropdown-toggle")
      .first();
    await expect(hamburgerBtn).toBeVisible();
    await hamburgerBtn.click();

    // Wait for the dropdown menu to appear.
    await expect(
      page.locator(".ironpad-toolbar-dropdown-menu").first()
    ).toBeVisible();

    // Start waiting for the download before clicking.
    const downloadPromise = page.waitForEvent("download");
    await page
      .locator(".ironpad-toolbar-dropdown-item", {
        hasText: "Download .ironpad",
      })
      .click();

    const download = await downloadPromise;

    // ── Verify the downloaded file ───────────────────────────────────────
    // Check the suggested filename ends with .ironpad.
    expect(download.suggestedFilename()).toMatch(/\.ironpad$/);

    // Save to a temp path and read the content.
    const tmpPath = path.join("/tmp", download.suggestedFilename());
    await download.saveAs(tmpPath);
    const content = fs.readFileSync(tmpPath, "utf-8");

    // Parse and validate structure.
    const notebook = JSON.parse(content);
    expect(notebook).toHaveProperty("version");
    expect(notebook).toHaveProperty("title");
    expect(notebook).toHaveProperty("cells");
    expect(typeof notebook.version).toBe("number");
    expect(typeof notebook.title).toBe("string");
    expect(Array.isArray(notebook.cells)).toBe(true);
    expect(notebook.cells.length).toBeGreaterThanOrEqual(1);

    // Clean up temp file.
    fs.unlinkSync(tmpPath);

    // Verify no JS errors occurred.
    expect(jsErrors).toEqual([]);
  });

  test("import button opens file chooser", async ({ page }) => {
    // Verify the import button triggers a file picker dialog.
    test.setTimeout(60_000);

    const jsErrors = trackJsErrors(page);

    // ── Navigate and wait for hydration ──────────────────────────────────
    await page.goto("/");
    await expect(page.locator(".ironpad-home")).toBeVisible();
    await page.waitForTimeout(3_000);

    // ── Click "Import Notebook" and verify file chooser fires ────────────
    const fileChooserPromise = page.waitForEvent("filechooser", {
      timeout: 10_000,
    });
    await page.locator("button", { hasText: "Import Notebook" }).click();
    const fileChooser = await fileChooserPromise;

    // Verify the file chooser accepts .ironpad files.
    expect(fileChooser.isMultiple()).toBe(false);

    // Verify the hidden file input was created with correct accept types.
    const hiddenInput = page.locator('input[type="file"]');
    const accept = await hiddenInput.getAttribute("accept");
    expect(accept).toContain(".ironpad");

    // Verify no JS errors occurred.
    expect(jsErrors).toEqual([]);
  });

  test("import valid .ironpad adds notebook to list", async ({ page }) => {
    // Drives the real Import Notebook UI end to end: file chooser → FileReader
    // → validation → IndexedDB import. Asserts the success toast (regression
    // coverage for the ToasterInjection owner fix — the toast used to be
    // swallowed by a panic outside the reactive owner) and the in-place list
    // refresh (no reload: the import updates the signal directly).
    test.setTimeout(60_000);

    const jsErrors = trackJsErrors(page);

    // ── Navigate to home page ────────────────────────────────────────────
    await page.goto("/");
    await expect(page.locator(".ironpad-home")).toBeVisible();
    await page.waitForTimeout(3_000);

    // ── Import a notebook via the UI file chooser ────────────────────────
    const fixture = makeFixture("E2E Import Test Notebook");
    const fixturePath = path.join("/tmp", "e2e-import-valid.ironpad");
    fs.writeFileSync(fixturePath, JSON.stringify(fixture));

    const fileChooserPromise = page.waitForEvent("filechooser", {
      timeout: 10_000,
    });
    await page.locator("button", { hasText: "Import Notebook" }).click();
    const fileChooser = await fileChooserPromise;
    await fileChooser.setFiles(fixturePath);

    // The success toast is the completion signal.
    await expect(page.getByText("Notebook Imported").first()).toBeVisible({
      timeout: 15_000,
    });
    // Scope to the card title: the toast body also contains the title, so a
    // bare getByText is a strict-mode violation.
    await expect(
      page.locator(".ironpad-notebook-card-title", {
        hasText: "E2E Import Test Notebook",
      })
    ).toBeVisible({ timeout: 10_000 });
    fs.unlinkSync(fixturePath);

    // ── Click the imported notebook and verify it opens ──────────────────
    await page
      .locator(".ironpad-notebook-card-title", {
        hasText: "E2E Import Test Notebook",
      })
      .click();
    await expect(page).toHaveURL(/\/local\/[a-f0-9-]+/);
    await expect(page.locator(".ironpad-editor")).toBeVisible({
      timeout: 10_000,
    });

    // Verify the notebook has the expected cell.
    await expect(page.locator(".ironpad-cell-card")).toHaveCount(1, {
      timeout: 10_000,
    });

    // Verify no JS errors occurred.
    expect(jsErrors).toEqual([]);
  });

  test("import invalid JSON does not add a notebook", async ({ page }) => {
    // Malformed JSON must be rejected with a visible "Import Failed" toast
    // (regression coverage for the ToasterInjection owner fix — the toast used
    // to be swallowed by a panic outside the reactive owner) and must NOT add
    // a notebook.
    test.setTimeout(60_000);

    // ── Navigate and wait for hydration ──────────────────────────────────
    await page.goto("/");
    await expect(page.locator(".ironpad-home")).toBeVisible();
    await page.waitForTimeout(3_000);

    // Count existing notebooks before the import attempt.
    const beforeCount = await page.evaluate(async () => {
      const nbs = await window.IronpadStorage.listNotebooks();
      return nbs.length;
    });

    // ── Attempt to import invalid JSON ───────────────────────────────────
    const invalidPath = path.join("/tmp", "e2e-import-invalid.ironpad");
    fs.writeFileSync(invalidPath, "this is not valid json {{{");

    const fileChooserPromise = page.waitForEvent("filechooser", {
      timeout: 10_000,
    });
    await page.locator("button", { hasText: "Import Notebook" }).click();
    const fileChooser = await fileChooserPromise;
    await fileChooser.setFiles(invalidPath);

    // The rejection toast is the positive completion signal; once it shows,
    // the validate-and-reject path has fully run.
    await expect(page.getByText("Import Failed").first()).toBeVisible({
      timeout: 15_000,
    });

    const afterCount = await page.evaluate(async () => {
      const nbs = await window.IronpadStorage.listNotebooks();
      return nbs.length;
    });
    expect(afterCount).toBe(beforeCount);

    // Clean up temp file.
    fs.unlinkSync(invalidPath);
  });

  test("import JSON missing required fields does not add a notebook", async ({
    page,
  }) => {
    // Same contract as the sibling "import invalid JSON" test: validation
    // rejects the file with a visible "Import Failed" toast and no notebook
    // is added.
    test.setTimeout(60_000);

    // ── Navigate and wait for hydration ──────────────────────────────────
    await page.goto("/");
    await expect(page.locator(".ironpad-home")).toBeVisible();
    await page.waitForTimeout(3_000);

    // Count existing notebooks before the import attempt.
    const beforeCount = await page.evaluate(async () => {
      const nbs = await window.IronpadStorage.listNotebooks();
      return nbs.length;
    });

    // ── Attempt to import JSON with missing "cells" field ────────────────
    const badFixture = { version: 1, title: "Missing Cells" };
    const badPath = path.join("/tmp", "e2e-import-bad-fields.ironpad");
    fs.writeFileSync(badPath, JSON.stringify(badFixture));

    const fileChooserPromise = page.waitForEvent("filechooser", {
      timeout: 10_000,
    });
    await page.locator("button", { hasText: "Import Notebook" }).click();
    const fileChooser = await fileChooserPromise;
    await fileChooser.setFiles(badPath);

    // The rejection toast is the positive completion signal (see the
    // invalid-JSON test).
    await expect(page.getByText("Import Failed").first()).toBeVisible({
      timeout: 15_000,
    });

    const afterCount = await page.evaluate(async () => {
      const nbs = await window.IronpadStorage.listNotebooks();
      return nbs.length;
    });
    expect(afterCount).toBe(beforeCount);

    // Clean up temp file.
    fs.unlinkSync(badPath);
  });
});
