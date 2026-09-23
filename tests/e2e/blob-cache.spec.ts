import { test, expect, Page } from "@playwright/test";
import { trackJsErrors } from "./helpers/errors";
import { menuClick } from "./helpers/menu";
import { setCellSource } from "./helpers/monaco";
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
    // Filtered: the immediate "Sharing…" progress toast coexists with it.
    await menuClick(page, "Share Immutable");
    const toastBody = page.locator(".ironpad-toast-body", {
      hasText: "/shared/",
    });
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

test.describe("Local blob store LRU touch", () => {
  test("a hit rewrites lastUsed only once it is a minute stale", async ({
    page,
  }) => {
    await page.goto("/");
    await page.waitForFunction(() => (window as any).IronpadStorage);
    const touch = await page.evaluate(async () => {
      const S = (window as any).IronpadStorage;
      const hash = `lru-touch-${crypto.randomUUID()}`;
      await S.putBlob(hash, new Uint8Array([0, 97, 115, 109]), null, null, 0);

      // Raw IndexedDB: storage.js has no lastUsed setter, and reading the
      // record back through getBlob would itself be the touch under test.
      const withStore = (
        mode: IDBTransactionMode,
        op: (store: IDBObjectStore) => IDBRequest,
      ) =>
        new Promise<any>((resolve, reject) => {
          const open = indexedDB.open("ironpad");
          open.onerror = () => reject(open.error);
          open.onsuccess = () => {
            const db = open.result;
            const req = op(db.transaction("blobs", mode).objectStore("blobs"));
            req.onsuccess = () => {
              db.close();
              resolve(req.result);
            };
            req.onerror = () => {
              db.close();
              reject(req.error);
            };
          };
        });
      const lastUsed = async () =>
        (await withStore("readonly", (s) => s.get(hash))).lastUsed;

      const record = await withStore("readonly", (s) => s.get(hash));
      record.lastUsed = Date.now() - 120_000;
      await withStore("readwrite", (s) => s.put(record));
      const stale = await lastUsed();

      await S.getBlob(hash);
      const afterFirst = await lastUsed();
      // Long enough that an unconditional touch would stamp a new value.
      await new Promise((r) => setTimeout(r, 20));
      const hit = await S.getBlob(hash);
      const afterSecond = await lastUsed();
      return { stale, afterFirst, afterSecond, hitLen: hit?.wasm?.length };
    });
    expect(touch.afterFirst, "a stale entry is touched").toBeGreaterThan(
      touch.stale,
    );
    expect(
      touch.afterSecond,
      "a fresh entry's hit does not rewrite the record",
    ).toBe(touch.afterFirst);
    expect(touch.hitLen, "the skipped touch still serves the blob").toBe(4);
  });
});
