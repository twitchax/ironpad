import { test, expect, Page } from "@playwright/test";
import { setCellSource } from "./helpers/monaco";
import { trackJsErrors } from "./helpers/errors";
import { createNotebook } from "./helpers/session";

/**
 * PRD-0047: static blob delivery — share-time blob snapshots (uat-001) and
 * the local IndexedDB blob cache with Force Recompile semantics (uat-002,
 * uat-003).
 */

const BASE = "http://localhost:3111";
const CELL_SOURCE = 'CellOutput::text(format!("{}", 41 + 1))';

/** Count compile_cell and /share-blobs/ traffic from `page` onward. */
function trackRequests(page: Page) {
  const compile: string[] = [];
  const shareBlobs: string[] = [];
  page.on("request", (req) => {
    const url = req.url();
    if (url.includes("compile_cell")) compile.push(url);
    if (url.includes("/share-blobs/")) shareBlobs.push(url);
  });
  return { compile, shareBlobs };
}

/** Add a cell, set its source, run it, and wait for the 42 output. */
async function addAndRunCell(page: Page): Promise<void> {
  await page.locator(".ironpad-add-cell-btn").first().click();
  const cell = page.locator(".ironpad-cell-card").first();
  await expect(cell).toBeVisible();
  await expect(cell.locator(".monaco-editor").first()).toBeVisible({
    timeout: 15_000,
  });
  await setCellSource(page, cell, CELL_SOURCE);
  await page.locator('button[title="Run cell"]').first().click();
  await expect(cell.locator(".ironpad-cell-status--success")).toBeVisible({
    timeout: 300_000,
  });
  await expect(cell.locator(".ironpad-output-display-text")).toContainText(
    "42"
  );
}

/** Toggle the gear menu's Force Recompile item, then close the menu. */
async function toggleForceRecompile(page: Page): Promise<void> {
  await page.locator('button[title="Notebook settings"]').click();
  await page
    .locator(".ironpad-toolbar-dropdown-item", { hasText: "Force Recompile" })
    .click();
  await page.locator('button[title="Notebook settings"]').click();
}

test.describe("Blob delivery (PRD-0047)", () => {
  test("shared notebook replays from blob snapshots with zero compiles", async ({
    page,
    browser,
  }) => {
    test.setTimeout(600_000);

    await createNotebook(page);
    await addAndRunCell(page);

    // Share via the hamburger menu; the success toast body carries the URL.
    await page.locator('button[title="Notebook menu"]').click();
    await page
      .locator(".ironpad-toolbar-dropdown-item", { hasText: "Share" })
      .click();
    const toastBody = page.locator(".ironpad-toast-body");
    await expect(toastBody).toContainText("/shared/", { timeout: 30_000 });
    const toastText = await toastBody.textContent();
    const match = toastText!.match(/\/shared\/([0-9a-f]{16})/);
    expect(match).not.toBeNull();

    // Fresh context: no local blob cache, no fingerprint memo — the only
    // no-compile path left is the share snapshot.
    const viewer = await browser.newContext();
    const viewerPage = await viewer.newPage();
    const requests = trackRequests(viewerPage);
    const jsErrors = trackJsErrors(viewerPage);

    await viewerPage.goto(`${BASE}/shared/${match![1]}`);
    await expect(viewerPage.locator(".view-only-notebook")).toBeVisible({
      timeout: 30_000,
    });
    await viewerPage.waitForTimeout(3_000); // hydration (suite convention)
    await viewerPage.locator(".run-all-button").click();
    await expect(
      viewerPage.locator(".view-only-timing-badge").first()
    ).toBeVisible({ timeout: 120_000 });
    await expect(viewerPage.locator(".view-only-notebook")).toContainText(
      "42"
    );

    expect(requests.shareBlobs.length).toBeGreaterThan(0);
    expect(requests.compile).toEqual([]);
    expect(jsErrors).toEqual([]);

    await viewer.close();
  });

  test("editor second run of an unchanged cell skips the compile round trip", async ({
    page,
  }) => {
    test.setTimeout(600_000);

    const jsErrors = trackJsErrors(page);
    await createNotebook(page);
    await addAndRunCell(page);

    const status = page.locator(".ironpad-cell-status").first();
    const before = await status.textContent();

    const requests = trackRequests(page);
    await page.locator('button[title="Run cell"]').first().click();

    // The badge re-renders with a fresh compile time when the run finishes.
    // A text collision with the previous time is possible in principle, so
    // this wait is best-effort — the meaning is carried by the assertions
    // below: a server trip in this window would land in requests.compile
    // regardless of timing.
    await expect(status)
      .not.toHaveText(before ?? "", { timeout: 15_000 })
      .catch(() => {});

    await expect(
      page.locator(".ironpad-cell-status--success").first()
    ).toBeVisible({ timeout: 60_000 });
    await expect(
      page.locator(".ironpad-output-display-text").first()
    ).toContainText("42");
    expect(requests.compile).toEqual([]);
    expect(jsErrors).toEqual([]);
  });

  test("Force Recompile hits the server, then cache mode serves the fresh blob locally", async ({
    page,
  }) => {
    test.setTimeout(600_000);

    await createNotebook(page);
    await addAndRunCell(page);

    // Fresh mode: the run MUST reach the server (force bypasses both the
    // local store and the server cache).
    await toggleForceRecompile(page);
    const compileRequest = page.waitForRequest(
      (r) => r.url().includes("compile_cell"),
      { timeout: 60_000 }
    );
    await page.locator('button[title="Run cell"]').first().click();
    await compileRequest;
    const cell = page.locator(".ironpad-cell-card").first();
    await expect(cell.locator(".ironpad-cell-status--compiling")).toBeHidden({
      timeout: 300_000,
    });
    await expect(cell.locator(".ironpad-cell-status--success")).toBeVisible({
      timeout: 60_000,
    });

    // Back to cache mode: the fresh result overwrote the local entry, so
    // the next run serves locally — zero compile traffic in the window.
    await toggleForceRecompile(page);
    const cached = trackRequests(page);
    await page.locator('button[title="Run cell"]').first().click();
    await page.waitForTimeout(10_000); // settle window; a server trip would land below
    await expect(cell.locator(".ironpad-cell-status--success")).toBeVisible();
    await expect(
      cell.locator(".ironpad-output-display-text")
    ).toContainText("42");
    expect(cached.compile).toEqual([]);
  });
});
