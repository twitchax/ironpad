import { test, expect } from "@playwright/test";
import { setCellSource } from "./helpers/monaco";
import { MENU } from "./helpers/mutable";
import { createNotebook, waitForPersistedCells } from "./helpers/session";

/**
 * PRD-0058: /local version history. Saves mint at most one snapshot per
 * five-minute bucket into a local IndexedDB ring; the History panel lists
 * them and Restore (confirm-gated) brings one back — after force-saving the
 * current version, so the restore itself is undoable.
 */


test.describe("Local version history (PRD-0058)", () => {
  test("edit, restore, and the restore itself is undoable", async ({
    page,
  }) => {
    test.setTimeout(120_000);

    await createNotebook(page);
    const notebookId = page.url().match(/\/local\/([a-f0-9-]+)/)![1];

    // Version A.
    await page.locator(".ironpad-add-cell-btn").first().click();
    const cell = page.locator(".ironpad-cell-card").first();
    await expect(cell.locator(".monaco-editor").first()).toBeVisible({
      timeout: 15_000,
    });
    await setCellSource(page, cell, "let version_a = 1;");
    await waitForPersistedCells(page, 1);
    // The save debounce is 1s; give the write a beat to land.
    await page.waitForTimeout(2_000);

    // Snapshot A explicitly. Ordinary saves mint at most one snapshot per
    // five-minute bucket — by design, so the ring holds PAST states rather
    // than a copy of every keystroke — and a whole e2e run fits inside one
    // bucket. snapshotNow is the same forced-capture the restore flow uses.
    await page.evaluate(
      (id) => (window as any).IronpadStorage.snapshotNow(id),
      notebookId,
    );

    // Version B: the current content diverges from snapshot A.
    await setCellSource(page, cell, "let version_b = 2;");
    await page.waitForTimeout(2_000);

    // The panel lists snapshots, newest first (A is newest: the notebook's
    // creation save also snapshotted the then-empty notebook).
    await page.locator(MENU).click();
    await page
      .locator(".ironpad-toolbar-dropdown-item", { hasText: "History" })
      .click();
    const panel = page.locator(".ironpad-history-panel");
    await expect(panel).toBeVisible();
    await expect(
      panel.locator(".ironpad-history-entry").first(),
    ).toBeVisible({ timeout: 10_000 });

    // The panel must be OPAQUE. It shipped reading `var(--ip-bg)`, a name the
    // palette never declared, so the whole background declaration was dropped
    // and the notebook showed straight through the snapshot list.
    await expect(panel).not.toHaveCSS(
      "background-color",
      "rgba(0, 0, 0, 0)",
    );

    // Restore version A (accept the confirm). Ends in a reload.
    page.on("dialog", (d) => d.accept());
    await panel.locator(".ironpad-history-restore").first().click();
    await expect(
      page.locator(".ironpad-toast-title", { hasText: "Restored" }),
    ).toBeVisible({ timeout: 30_000 });
    await expect(
      page.locator(".ironpad-cell-card .monaco-editor .view-lines").first(),
    ).toContainText("version_a", { timeout: 30_000 });

    // Undoable: the pre-restore version B was force-snapshotted, so the
    // ring holds it as the newest entry.
    const history = await page.evaluate(
      (id) => (window as any).IronpadStorage.listHistory(id),
      notebookId,
    );
    expect(history.length).toBeGreaterThanOrEqual(2);
    const newest = await page.evaluate(
      async ([id, savedAt]) =>
        (window as any).IronpadStorage.getHistorySnapshot(id, savedAt),
      [notebookId, history[0].savedAt] as const,
    );
    expect(newest, "the pre-restore version is recoverable").toContain(
      "version_b",
    );

    // Deleting the notebook deletes its history.
    await page.locator(MENU).click();
    await page
      .locator(".ironpad-toolbar-dropdown-item", { hasText: "Delete" })
      .click();
    await expect(page).toHaveURL(/\/$/, { timeout: 15_000 });
    const afterDelete = await page.evaluate(
      (id) => (window as any).IronpadStorage.listHistory(id),
      notebookId,
    );
    expect(afterDelete).toEqual([]);
    // ...and the notebook itself: the editor's local Delete removed the
    // IndexedDB record, not just the history ring.
    const record = await page.evaluate(
      (id) => (window as any).IronpadStorage.getNotebook(id),
      notebookId,
    );
    expect(record, "the deleted notebook is gone from local storage").toBeNull();
  });

  test("Restore flushes typing still inside the save debounce into the pre-restore snapshot", async ({
    page,
  }) => {
    test.setTimeout(120_000);

    await createNotebook(page);
    const notebookId = page.url().match(/\/local\/([a-f0-9-]+)/)![1];

    await page.locator(".ironpad-add-cell-btn").first().click();
    const cell = page.locator(".ironpad-cell-card").first();
    await expect(cell.locator(".monaco-editor").first()).toBeVisible({
      timeout: 15_000,
    });
    await setCellSource(page, cell, "let saved_before = 1;");
    await waitForPersistedCells(page, 1);
    await page.waitForTimeout(2_000);
    await page.evaluate(
      (id) => (window as any).IronpadStorage.snapshotNow(id),
      notebookId,
    );

    // Open the panel BEFORE typing, so the Restore click lands well inside
    // the 1s save debounce: the edit below exists only in Monaco and the
    // cell's dirty flag, never in the model or IndexedDB, until a flush.
    await page.locator(MENU).click();
    await page
      .locator(".ironpad-toolbar-dropdown-item", { hasText: "History" })
      .click();
    const panel = page.locator(".ironpad-history-panel");
    const restore = panel.locator(".ironpad-history-restore").first();
    await expect(restore).toBeVisible({ timeout: 10_000 });

    page.on("dialog", (d) => d.accept());
    await setCellSource(page, cell, "let typed_inside_the_debounce = 2;");
    await restore.click();
    await expect(
      page.locator(".ironpad-toast-title", { hasText: "Restored" }),
    ).toBeVisible({ timeout: 30_000 });

    // The reload killed the pending debounce timer, so the only way that
    // text survives is the pre-restore flush putting it in the snapshot
    // that makes the restore undoable.
    const history = await page.evaluate(
      (id) => (window as any).IronpadStorage.listHistory(id),
      notebookId,
    );
    const newest = await page.evaluate(
      async ([id, savedAt]) =>
        (window as any).IronpadStorage.getHistorySnapshot(id, savedAt),
      [notebookId, history[0].savedAt] as const,
    );
    expect(
      newest,
      "the last second of typing is in the pre-restore snapshot",
    ).toContain("typed_inside_the_debounce");
  });

  // The ring's two limits, driven straight through IronpadStorage: the
  // save path reads keys alone now, so the cap and the bucket are exactly
  // what it has to get right without ever loading an entry.

  test("the ring holds the newest 30 snapshots", async ({ page }) => {
    await page.goto("/");
    await page.waitForFunction(() => (window as any).IronpadStorage);
    const ring = await page.evaluate(async () => {
      const S = (window as any).IronpadStorage;
      const id = crypto.randomUUID();
      const now = new Date().toISOString();
      await S.saveNotebook({
        version: 1,
        id,
        title: "ring",
        created_at: now,
        updated_at: now,
        cells: [],
      });
      const [first] = await S.listHistory(id);
      for (let i = 0; i < 32; i++) await S.snapshotNow(id);
      const savedAts = (await S.listHistory(id)).map(
        (e: { savedAt: number }) => e.savedAt,
      );
      await S.deleteNotebook(id);
      return { first: first.savedAt, savedAts };
    });
    expect(ring.savedAts).toHaveLength(30);
    expect(ring.savedAts[0], "newest first").toBe(Math.max(...ring.savedAts));
    expect(
      ring.savedAts.every((v, i, a) => i === 0 || a[i - 1] > v),
      "strictly newest-first",
    ).toBe(true);
    expect(ring.savedAts, "the oldest snapshot is the one pruned").not.toContain(
      ring.first,
    );
  });

  test("saves inside the five-minute bucket add no snapshot", async ({
    page,
  }) => {
    await page.goto("/");
    await page.waitForFunction(() => (window as any).IronpadStorage);
    const counts = await page.evaluate(async () => {
      const S = (window as any).IronpadStorage;
      const id = crypto.randomUUID();
      const now = new Date().toISOString();
      const nb = {
        version: 1,
        id,
        title: "bucket",
        created_at: now,
        updated_at: now,
        cells: [],
      };
      await S.saveNotebook(nb);
      const before = (await S.listHistory(id)).length;
      await S.saveNotebook({ ...nb, title: "again" });
      await S.saveNotebook({ ...nb, title: "and again" });
      const after = (await S.listHistory(id)).length;
      await S.deleteNotebook(id);
      const afterDelete = (await S.listHistory(id)).length;
      return { before, after, afterDelete };
    });
    expect(counts.before, "the first save snapshots").toBe(1);
    expect(counts.after, "later saves in the bucket do not").toBe(1);
    expect(counts.afterDelete, "the ring dies with the notebook").toBe(0);
  });
});
