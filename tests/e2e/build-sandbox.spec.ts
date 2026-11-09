import { test, expect, Page, Locator } from "@playwright/test";
import { setCellSource } from "./helpers/monaco";
import { trackJsErrors } from "./helpers/errors";

/**
 * The build sandbox (PRD-0030's revised decision): in the deploy image the
 * server runs as root, but every cargo process it starts runs as an
 * unprivileged build user, so code a cell's author chose (a build script, a
 * proc macro, or just `include_bytes!`) cannot read the accounts database.
 *
 * Container only: `cargo make docker-uat` sets IRONPAD_DOCKER_UAT. The paths
 * below exist only in the image's layout, so against a dev server the first
 * test would fail its build for the wrong reason (no such file) and prove
 * nothing.
 */
test.describe("Build sandbox (docker-uat only)", () => {
  test.skip(
    !process.env.IRONPAD_DOCKER_UAT,
    "runs against the docker-uat container only"
  );

  /** A fresh notebook's first cell, ready for source. */
  async function firstCell(page: Page): Promise<Locator> {
    await page.goto("/");
    await expect(page.locator(".ironpad-home")).toBeVisible();
    await page.waitForTimeout(3_000); // hydration (suite convention)
    await page.locator("button", { hasText: "New Notebook" }).click();
    await expect(page).toHaveURL(/\/local\/[a-f0-9-]+/);
    await page.locator(".ironpad-add-cell-btn").first().click();
    const cell = page.locator(".ironpad-cell-card").first();
    await expect(cell.locator(".monaco-editor").first()).toBeVisible({
      timeout: 15_000,
    });
    return cell;
  }

  test("a cell cannot embed the accounts database", async ({ page }) => {
    test.setTimeout(300_000);
    const cell = await firstCell(page);
    await setCellSource(
      page,
      cell,
      'let wal: &[u8] = include_bytes!("/ironpad/data/ironpad.db/wal/00000000000000000000.wal");\n' +
        'CellOutput::text(format!("{}", wal.len()))'
    );
    await page.locator('button[title="Run cell"]').first().click();

    await expect(cell.locator(".ironpad-cell-status--error")).toBeVisible({
      timeout: 240_000,
    });
    // Refused by the filesystem, not missing: the file exists in the image
    // (docker-uat checks it as root), so this is the build user's denial.
    await expect(cell.locator(".ironpad-error-panel")).toContainText(
      /permission denied/i
    );
  });

  test("a cell can still embed a world-readable file", async ({ page }) => {
    // Positive control for the test above: include_bytes! itself works, so
    // the denial there is about WHICH file, not about the macro.
    test.setTimeout(300_000);
    const jsErrors = trackJsErrors(page);
    const cell = await firstCell(page);
    await setCellSource(
      page,
      cell,
      'let host: &[u8] = include_bytes!("/etc/hostname");\n' +
        'CellOutput::text(format!("read {} bytes", host.len()))'
    );
    await page.locator('button[title="Run cell"]').first().click();

    await expect(cell.locator(".ironpad-cell-status--success")).toBeVisible({
      timeout: 240_000,
    });
    await expect(cell.locator(".ironpad-output-display-text")).toContainText(
      /read [1-9]\d* bytes/
    );
    expect(jsErrors).toEqual([]);
  });

  test("a cell with a crates.io dependency still builds", async ({ page }) => {
    // The build user writes the registry, the target dir and the cell's
    // Cargo.lock: a fresh dependency exercises all three, through the proxy.
    test.setTimeout(300_000);
    const jsErrors = trackJsErrors(page);
    const cell = await firstCell(page);

    const panes = cell.locator(".ironpad-cell-editor-pane");
    await cell.locator(".ironpad-cell-tab", { hasText: "Cargo.toml" }).click();
    await expect(panes.nth(1).locator(".monaco-editor")).toBeVisible({
      timeout: 15_000,
    });
    await setCellSource(page, panes.nth(1), '[dependencies]\nitoa = "1"\n');
    await cell.locator(".ironpad-cell-tab", { hasText: "Code" }).click();
    await setCellSource(
      page,
      panes.nth(0),
      "let mut buf = itoa::Buffer::new();\n" +
        'CellOutput::text(format!("itoa says {}", buf.format(1234)))'
    );
    await page.locator('button[title="Run cell"]').first().click();

    await expect(cell.locator(".ironpad-cell-status--success")).toBeVisible({
      timeout: 240_000,
    });
    await expect(cell.locator(".ironpad-output-display-text")).toContainText(
      "itoa says 1234"
    );
    expect(jsErrors).toEqual([]);
  });
});
