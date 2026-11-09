import { test, expect, type Page } from "@playwright/test";

/**
 * A notebook save resolves only once IndexedDB has COMMITTED it.
 *
 * A request's `success` event is not a commit. Chromium decides storage quota
 * at commit and reports a refusal as a transaction abort AFTER the put has
 * already succeeded, so a save that resolved on `success` reported "saved"
 * while the record kept its old content. Every caller (autosave, Fork, New
 * Notebook, Restore, Import) relies on that promise rejecting.
 *
 * Exhausting a real quota takes ~30s of Chromium's cached headroom to bite, so
 * this aborts the transaction from the put's own success handler instead,
 * which is the same event order a quota refusal produces.
 */

async function openHome(page: Page): Promise<void> {
  await page.addInitScript(() => {
    const put = IDBObjectStore.prototype.put;
    IDBObjectStore.prototype.put = function (
      this: IDBObjectStore,
      ...args: Parameters<IDBObjectStore["put"]>
    ) {
      const request = put.apply(this, args);
      const w = window as unknown as { __abortNotebookPut?: boolean };
      if (w.__abortNotebookPut && this.name === "notebooks") {
        request.addEventListener("success", () => this.transaction.abort());
      }
      return request;
    };
  });
  await page.goto("/");
  await expect(page.locator(".ironpad-home")).toBeVisible({ timeout: 15_000 });
}

/** Save v1, then save v2 with or without the abort armed; report the outcome. */
async function saveTwice(page: Page, abortSecond: boolean) {
  return page.evaluate(async (abort) => {
    const w = window as unknown as {
      __abortNotebookPut?: boolean;
      IronpadStorage: {
        saveNotebook: (nb: object) => Promise<void>;
        getNotebook: (id: string) => Promise<{ title: string } | null>;
        deleteNotebook: (id: string) => Promise<void>;
      };
    };
    const storage = w.IronpadStorage;
    const nb = {
      version: 1,
      id: crypto.randomUUID(),
      title: "v1",
      created_at: "2026-01-01T00:00:00Z",
      updated_at: "2026-01-01T00:00:00Z",
      cells: [],
    };
    await storage.saveNotebook(nb);

    w.__abortNotebookPut = abort;
    let rejected = false;
    try {
      await storage.saveNotebook({ ...nb, title: "v2" });
    } catch {
      rejected = true;
    } finally {
      w.__abortNotebookPut = false;
    }

    const stored = await storage.getNotebook(nb.id);
    await storage.deleteNotebook(nb.id);
    return { rejected, title: stored?.title ?? null };
  }, abortSecond);
}

test.describe("IndexedDB durability", () => {
  test("a save whose transaction aborts at commit rejects", async ({ page }) => {
    await openHome(page);
    expect(await saveTwice(page, true)).toEqual({ rejected: true, title: "v1" });
  });

  test("a save that commits resolves and persists", async ({ page }) => {
    // Positive control: the same harness with the abort unarmed.
    await openHome(page);
    expect(await saveTwice(page, false)).toEqual({ rejected: false, title: "v2" });
  });
});
