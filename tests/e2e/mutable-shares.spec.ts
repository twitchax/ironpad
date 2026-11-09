import fs from "fs";
import path from "path";

import { test, expect, APIRequestContext, Page } from "@playwright/test";
import { loginTestUser, logout } from "./helpers/auth";
import { setCellSource } from "./helpers/monaco";
import {
  expectLiveCard,
  expectOwnerEditor,
  menuClick,
  shareMutable,
} from "./helpers/mutable";
import { openMetadataPanel } from "./helpers/metadata";
import { createNotebook, renameNotebook } from "./helpers/session";

/**
 * PRD-0054: server-authoritative mutable shares with a draft/published
 * split. ONE address per published notebook: /mutable/{id} is the view-only
 * reader of PUBLISHED for everyone, and the live editor over the server
 * DRAFT for the owner (auto-swapped on hydrate). Edits autosave to the
 * draft; the toolbar Push button promotes draft → published; readers never
 * see a draft. Ownership is the GitHub session's OWNER grant (PRD-0053),
 * minted here via the env-gated test login (helpers/auth.ts).
 *
 * Also the /mutable half of the PRD-0050 unfurl contract (social-preview
 * .spec.ts owns /public and /shared): metadata assertions MUST run against
 * raw response bodies via `request`, never the hydrated DOM — unfurlers run
 * no JavaScript.
 */

const BASE = "http://localhost:3111";

/** Comfortable margin over the 1.5s draft-autosave debounce + round trip. */
const DRAFT_SETTLE_MS = 3_000;

/** Value of a `<meta>` whose `property` or `name` is `key`, from raw HTML. */
function metaContent(html: string, key: string): string | null {
  const pattern = new RegExp(
    `<meta[^>]+(?:property|name)="${key.replace(
      /[.*+?^${}()|[\]\\]/g,
      "\\$&",
    )}"[^>]*>`,
    "i",
  );
  const tag = html.match(pattern)?.[0];
  if (!tag) return null;
  return tag.match(/content="([^"]*)"/i)?.[1] ?? null;
}

/** Raw SSR body — what an unfurler sees. Asserts the resolve is a 200. */
async function rawHtml(
  request: APIRequestContext,
  path: string,
): Promise<string> {
  const res = await request.get(`${BASE}${path}`);
  expect(res.status()).toBe(200);
  return res.text();
}

/**
 * A published (Share Mutable), clean notebook with one code cell, signed in
 * as `login`, its editor mounted at /mutable/{id}. Returns the id.
 */
async function publishedNotebookWithCell(
  page: Page,
  login: string,
  title: string,
): Promise<string> {
  await loginTestUser(page, login);
  await createNotebook(page);
  // Rename BEFORE adding a cell: a new cell steals focus when its Monaco
  // mounts (pending_focus_cell), which would hijack mid-flight title typing.
  await renameNotebook(page, title);
  await page.locator(".ironpad-add-cell-btn").first().click();
  await expect(
    page.locator(".ironpad-cell-card .monaco-editor").first(),
  ).toBeVisible({ timeout: 15_000 });
  const id = await shareMutable(page);
  await expect(
    page.locator(".ironpad-cell-card .monaco-editor").first(),
  ).toBeVisible({ timeout: 30_000 });
  return id;
}

/** The first cell's rendered editor text. */
const firstCellLines = (page: Page) =>
  page.locator(".ironpad-cell-card .monaco-editor .view-lines").first();

test.describe("Mutable shares (PRD-0054, draft/published)", () => {
  test("publish, draft invisibly, push: the one-URL owner lifecycle", async ({
    page,
    browser,
    request,
  }) => {
    test.setTimeout(150_000);
    await loginTestUser(page, "alice");
    await createNotebook(page);
    await renameNotebook(page, "Lifecycle One");

    const shareId = await shareMutable(page);

    // Fresh share: nothing to push.
    const push = page.locator(".ironpad-push-button");
    await expect(push).toBeDisabled();
    await expect(push).toHaveText(/Published/);

    // An edit arms Push and autosaves to the DRAFT.
    await renameNotebook(page, "Lifecycle One v2");
    await expect(push).toBeEnabled({ timeout: 10_000 });
    await expect(push).toHaveText(/Push/);
    await page.waitForTimeout(DRAFT_SETTLE_MS); // draft write lands

    // Readers (anonymous, fresh context) still see PUBLISHED — the draft
    // must be invisible, in the DOM and in the unfurl body alike.
    const anon = await browser.newContext();
    try {
      const reader = await anon.newPage();
      await reader.goto(`/mutable/${shareId}`);
      await expect(reader.locator(".view-only-title")).toHaveText(
        "Lifecycle One",
        { timeout: 30_000 },
      );
      await expect(reader.locator(".mutable-attribution")).toContainText(
        "@alice",
      );
      // No owner chrome for readers.
      await reader.waitForTimeout(1_500);
      await expect(reader.locator(".ironpad-push-button")).toHaveCount(0);
    } finally {
      await anon.close();
    }
    const html = await rawHtml(request, `/mutable/${shareId}`);
    expect(metaContent(html, "og:title")).toBe("Lifecycle One");

    // Push promotes the draft; the button grays again.
    await push.click();
    await expect(
      page.locator(".ironpad-toast-title", { hasText: "Pushed" }),
    ).toBeVisible({ timeout: 30_000 });
    await expect(push).toBeDisabled({ timeout: 15_000 });

    // Readers and unfurlers now see the promoted content.
    const pushed = await rawHtml(request, `/mutable/${shareId}`);
    expect(metaContent(pushed, "og:title")).toBe("Lifecycle One v2");
    await expectLiveCard(request, shareId);
  });

  test("the reader hydrates in place instead of being rebuilt", async ({
    page,
    browser,
  }) => {
    // The ownership probe's first run used to write `None` over `None`, which
    // still notifies, so the page's outer view threw away the reader it had
    // just hydrated and built a new one: every Monaco editor, saved output and
    // the rail's observed anchors, on every load. The owner swap that the
    // probe exists for is covered by `shareMutable` (expectOwnerEditor).
    test.setTimeout(150_000);
    await loginTestUser(page, "alice");
    await createNotebook(page);
    await renameNotebook(page, "Hydrate In Place");
    const shareId = await shareMutable(page);

    const anon = await browser.newContext();
    try {
      const reader = await anon.newPage();
      await reader.addInitScript(() => {
        const w = window as unknown as { __readerRemovals: number };
        w.__readerRemovals = 0;
        document.addEventListener("DOMContentLoaded", () => {
          new MutationObserver((records) => {
            for (const record of records) {
              for (const node of Array.from(record.removedNodes)) {
                if (
                  node instanceof Element &&
                  (node.matches(".view-only-notebook") ||
                    node.querySelector(".view-only-notebook"))
                ) {
                  w.__readerRemovals++;
                }
              }
            }
          }).observe(document.documentElement, { childList: true, subtree: true });
        });
      });
      await reader.goto(`/mutable/${shareId}`);
      await expect(reader.locator(".view-only-title")).toHaveText(
        "Hydrate In Place",
        { timeout: 30_000 },
      );
      await reader.waitForTimeout(5_000);
      const removals = await reader.evaluate(
        () => (window as unknown as { __readerRemovals: number }).__readerRemovals,
      );
      expect(removals, "the hydrated reader was torn down").toBe(0);
    } finally {
      await anon.close();
    }
  });

  test("the draft is shared across the owner's devices; others never see it", async ({
    page,
    browser,
  }) => {
    test.setTimeout(150_000);
    await loginTestUser(page, "alice");
    await createNotebook(page);
    await renameNotebook(page, "Device One");
    const shareId = await shareMutable(page);

    // Draft an edit on device one and let the autosave land.
    await renameNotebook(page, "Device One WIP");
    await expect(page.locator(".ironpad-push-button")).toBeEnabled();
    await page.waitForTimeout(DRAFT_SETTLE_MS);

    // Device two, same account: the same URL opens the same DRAFT in the
    // editor — cross-device sync is the server draft itself (PRD-0054).
    const sameOwner = await browser.newContext();
    try {
      const p2 = await sameOwner.newPage();
      await loginTestUser(p2, "alice");
      await p2.goto(`/mutable/${shareId}`);
      await expectOwnerEditor(p2);
      await expect(p2.locator(".ironpad-notebook-title--editable")).toHaveText(
        "Device One WIP",
        { timeout: 15_000 },
      );
      // The server said dirty=true, so Push arrives armed.
      await expect(p2.locator(".ironpad-push-button")).toBeEnabled();

      // Pushing from device two publishes the shared draft.
      await p2.locator(".ironpad-push-button").click();
      await expect(
        p2.locator(".ironpad-toast-title", { hasText: "Pushed" }),
      ).toBeVisible({ timeout: 30_000 });
    } finally {
      await sameOwner.close();
    }

    // A different account gets the reader, not the editor.
    const other = await browser.newContext();
    try {
      const p3 = await other.newPage();
      await loginTestUser(p3, "bob");
      await p3.goto(`/mutable/${shareId}`);
      await expect(p3.locator(".view-only-notebook")).toBeVisible({
        timeout: 30_000,
      });
      await p3.waitForTimeout(1_500); // would-be editor swap must not happen
      await expect(p3.locator(".ironpad-push-button")).toHaveCount(0);
      await expect(p3.locator(".view-only-title")).toHaveText("Device One WIP");
    } finally {
      await other.close();
    }
  });

  test("discard draft reverts to published; view-as-reader round-trips", async ({
    page,
  }) => {
    test.setTimeout(150_000);
    await loginTestUser(page, "alice");
    await createNotebook(page);
    await renameNotebook(page, "Discard Base");
    const shareId = await shareMutable(page);

    // Draft an edit, then throw it away.
    await renameNotebook(page, "Discard Scratch");
    await expect(page.locator(".ironpad-push-button")).toBeEnabled();
    await page.waitForTimeout(DRAFT_SETTLE_MS);
    page.on("dialog", (d) => d.accept());
    await menuClick(page, "Discard Draft");
    // Ends in a reload; the toast rides sessionStorage across it.
    await expect(
      page.locator(".ironpad-toast-title", { hasText: "Draft Discarded" }),
    ).toBeVisible({ timeout: 30_000 });
    await expectOwnerEditor(page);
    await expect(page.locator(".ironpad-notebook-title--editable")).toHaveText(
      "Discard Base",
      { timeout: 15_000 },
    );
    await expect(page.locator(".ironpad-push-button")).toBeDisabled();

    // View Published pins the published reader for the owner, with a way
    // back into the editor.
    await menuClick(page, "View Published");
    await expect(page).toHaveURL(/view=reader/, { timeout: 15_000 });
    await expect(page.locator(".view-only-notebook")).toBeVisible({
      timeout: 30_000,
    });
    await expect(page.locator(".ironpad-push-button")).toHaveCount(0);
    await page.locator(".view-only-edit-button").click();
    await expectOwnerEditor(page);
  });

  // The other side of review bug-server-fns-9: Preview now persists a cell
  // flush, but only one that moved text, so previewing a clean published
  // notebook must not create a draft and arm Push.
  test("previewing a clean published notebook leaves Push disabled", async ({
    page,
  }) => {
    test.setTimeout(150_000);
    await loginTestUser(page, "alice");
    await createNotebook(page);
    await renameNotebook(page, "Preview Clean");
    await page.locator(".ironpad-add-cell-btn").first().click();
    await expect(page.locator(".ironpad-cell-card")).toHaveCount(1);
    await shareMutable(page);
    await expect(
      page.locator(".ironpad-cell-card .monaco-editor").first(),
    ).toBeVisible({ timeout: 15_000 });
    const push = page.locator(".ironpad-push-button");
    await expect(push).toBeDisabled();

    await page.locator('button[aria-label="Preview"]').click();
    await expect(page.locator(".view-only-notebook")).toBeVisible({
      timeout: 10_000,
    });
    await page.locator('button[aria-label="Edit"]').click();
    await expect(page.locator(".ironpad-cell-card")).toHaveCount(1, {
      timeout: 10_000,
    });
    await page.waitForTimeout(DRAFT_SETTLE_MS);
    await expect(push).toBeDisabled();
  });

  test("a mutable share unfurls from the raw body with unlisted robots", async ({
    page,
    request,
  }) => {
    test.setTimeout(120_000);
    await loginTestUser(page, "alice");
    await createNotebook(page);
    const title = `Mutable unfurl ${Date.now()}`;
    await renameNotebook(page, title);
    const shareId = await shareMutable(page);

    const html = await rawHtml(request, `/mutable/${shareId}`);
    // SsrMode::Async on this route is load-bearing: under streaming SSR the
    // head flushes before the Resource resolves, and the tags then exist in
    // devtools but not for any unfurler. Only the raw body proves it.
    expect(metaContent(html, "og:title")).toBe(title);
    expect(metaContent(html, "og:url")).toBe(`${BASE}/mutable/${shareId}`);
    expect(metaContent(html, "og:image")).toBe(
      `${BASE}/og/mutable/${shareId}.png`,
    );
    // Unlisted, not secret: noindex on the page rather than a robots.txt
    // Disallow, because several unfurlers honour robots.txt.
    expect(metaContent(html, "robots")).toContain("noindex");
    await expectLiveCard(request, shareId);

    // The home Published group lists it, linking to the one URL.
    await page.goto("/");
    await expect(
      page.locator(`a[href="/mutable/${shareId}"]`),
    ).toBeVisible({ timeout: 15_000 });
  });

  test("push aborts loudly when the draft save fails, and recovers", async ({
    page,
    request,
  }) => {
    test.setTimeout(150_000);
    await loginTestUser(page, "alice");
    await createNotebook(page);
    await renameNotebook(page, "Push Fail Base");
    const shareId = await shareMutable(page);

    // Fault injection: every draft save fails at the network layer. The
    // debounce autosave surfaces the visible failure state (uat-006), and a
    // Push must refuse to promote — the server never received the edit, so
    // promoting would publish stale content behind a success toast.
    await page.route(
      (url) => url.pathname.includes("save_mutable_draft"),
      (route) => route.abort(),
    );
    await renameNotebook(page, "Push Fail v2");
    const push = page.locator(".ironpad-push-button");
    await expect(push).toBeEnabled({ timeout: 10_000 });
    await expect(page.locator(".ironpad-draft-indicator")).toHaveText(
      /Draft not saved/,
      { timeout: 15_000 },
    );

    // The indicator sits AFTER Push. Its text appears and disappears on every
    // autosave, and ahead of the button that reflow slid Push out from under
    // a cursor already on its way down.
    const pushLeft = await push.evaluate(
      (el) => el.getBoundingClientRect().left,
    );
    const indicatorLeft = await page
      .locator(".ironpad-draft-indicator")
      .evaluate((el) => el.getBoundingClientRect().left);
    expect(indicatorLeft).toBeGreaterThan(pushLeft);

    await push.click();
    await expect(
      page.locator(".ironpad-toast-title", { hasText: "Push Failed" }),
    ).toBeVisible({ timeout: 30_000 });
    // Nothing was promoted and the button stays armed.
    await expect(push).toBeEnabled();
    await expect(push).toHaveText(/Push/);
    const stale = await rawHtml(request, `/mutable/${shareId}`);
    expect(metaContent(stale, "og:title")).toBe("Push Fail Base");

    // Network restored: the same Push flushes the draft and promotes it.
    await page.unrouteAll();
    await push.click();
    await expect(
      page.locator(".ironpad-toast-title", { hasText: "Pushed" }),
    ).toBeVisible({ timeout: 30_000 });
    await expect(push).toBeDisabled({ timeout: 15_000 });
    const pushed = await rawHtml(request, `/mutable/${shareId}`);
    expect(metaContent(pushed, "og:title")).toBe("Push Fail v2");
  });

  test("download .ironpad works from the mutable editor", async ({ page }) => {
    test.setTimeout(150_000);
    await loginTestUser(page, "alice");
    await createNotebook(page);
    // Rename BEFORE adding a cell: a new cell steals focus when its Monaco
    // mounts (pending_focus_cell), which would hijack mid-flight title typing.
    await renameNotebook(page, "Download Mutable");
    await page.locator(".ironpad-add-cell-btn").first().click();
    await expect(page.locator(".ironpad-cell-card")).toHaveCount(1);
    await shareMutable(page);

    // Regression: Download used to read the IndexedDB record, which Share
    // Mutable deletes — the menu item silently did nothing in this editor.
    const downloadPromise = page.waitForEvent("download");
    await menuClick(page, "Download .ironpad");
    const download = await downloadPromise;
    expect(download.suggestedFilename()).toMatch(/\.ironpad$/);
    const tmpPath = path.join("/tmp", download.suggestedFilename());
    await download.saveAs(tmpPath);
    const notebook = JSON.parse(fs.readFileSync(tmpPath, "utf-8"));
    fs.unlinkSync(tmpPath);
    expect(notebook.title).toBe("Download Mutable");
    expect(notebook.cells.length).toBe(1);
  });

  test("push keeps a keystroke made inside the debounce window", async ({
    page,
  }) => {
    test.setTimeout(150_000);
    await loginTestUser(page, "alice");
    await createNotebook(page);
    // Rename BEFORE adding a cell: a new cell steals focus when its Monaco
    // mounts (pending_focus_cell), which would hijack mid-flight title typing.
    await renameNotebook(page, "Push Flush");
    await page.locator(".ironpad-add-cell-btn").first().click();
    await expect(page.locator(".ironpad-cell-card")).toHaveCount(1);
    await shareMutable(page);
    await expect(
      page.locator(".ironpad-cell-card .monaco-editor").first(),
    ).toBeVisible({ timeout: 15_000 });

    // Edit a cell and push IMMEDIATELY — inside the ~1s editor->model
    // debounce. Push promotes whatever the SERVER holds, so without the
    // flush-before-serialize discipline this keystroke would be missing from
    // the copy readers get, behind a success toast.
    //
    // This assertion used to ride Unpublish, whose local save was for one
    // moment the only surviving copy of the notebook (PRD-0064 removed that
    // moment, and the flow with it). Push is where promoting stale content
    // is still the failure.
    await setCellSource(
      page,
      page.locator(".ironpad-cell-card").first(),
      "let flushed_before_push = 42;",
    );
    await page.locator(".ironpad-push-button").click();
    await expect(
      page.locator(".ironpad-toast-title", { hasText: "Pushed" }),
    ).toBeVisible({ timeout: 30_000 });

    // The PUBLISHED copy carries it. Asserted through View Published, which
    // renders `notebook_json` rather than the draft — the raw SSR body
    // cannot answer this, since a view-only code cell SSRs as an empty
    // Monaco container and only fills in on hydrate.
    await menuClick(page, "View Published");
    await expect(page.locator(".view-only-notebook")).toBeVisible({
      timeout: 30_000,
    });
    await expect(
      page.locator(".view-only-notebook .monaco-editor .view-lines").first(),
    ).toContainText("flushed_before_push", { timeout: 30_000 });
  });

  test("discard wins over autosaves that came due behind its confirm", async ({
    page,
  }) => {
    // The cell debounce has fired and the draft save is scheduled when the
    // menu opens; the confirm then blocks long enough for that save to come
    // due, and it used to fire 3ms after the discard POST, putting the
    // discarded edit back under a "Draft Discarded" toast.
    test.setTimeout(150_000);
    await publishedNotebookWithCell(page, "dina", "Discard Race");
    await setCellSource(
      page,
      page.locator(".ironpad-cell-card").first(),
      "let discarded_edit = 1;",
    );
    await page.waitForTimeout(1_200);
    page.once("dialog", async (dialog) => {
      await new Promise((resolve) => setTimeout(resolve, 1_500));
      await dialog.accept();
    });
    await menuClick(page, "Discard Draft");
    await expect(
      page.locator(".ironpad-toast-title", { hasText: "Draft Discarded" }),
    ).toBeVisible({ timeout: 30_000 });

    // Twice: the first load after the reload, then a cold one, so a save
    // that landed after the reload's read is caught too.
    const push = page.locator(".ironpad-push-button");
    for (const reload of [false, true]) {
      if (reload) await page.reload();
      await expectOwnerEditor(page);
      await expect(firstCellLines(page)).toContainText("42", {
        timeout: 30_000,
      });
      await expect(firstCellLines(page)).not.toContainText("discarded_edit");
      await expect(push).toHaveText(/Published/);
      await expect(push).toBeDisabled();
    }
  });

  test("a signed-out autosave says so, and lands after signing back in", async ({
    page,
    context,
  }) => {
    // "sign in with GitHub" used to count as transient: the indicator read
    // "Draft not saved; retrying" forever and nothing said to sign in.
    test.setTimeout(150_000);
    await publishedNotebookWithCell(page, "sam", "Lapsed Session");
    const other = await context.newPage();
    await logout(other);
    await setCellSource(
      page,
      page.locator(".ironpad-cell-card").first(),
      "let typed_while_signed_out = 1;",
    );
    const indicator = page.locator(".ironpad-draft-indicator");
    await expect(indicator).toHaveText("Draft not saved: signed out", {
      timeout: 15_000,
    });
    await expect(
      page.locator(".ironpad-toast-body", { hasText: "new tab" }),
    ).toHaveCount(1);

    // Still retried: signing in again in another tab is the remedy.
    await loginTestUser(other, "sam");
    await expect(indicator).toHaveText("", { timeout: 20_000 });
    await page.reload();
    await expectOwnerEditor(page);
    await expect(firstCellLines(page)).toContainText("typed_while_signed_out", {
      timeout: 30_000,
    });
  });

  test("explicit saves report a draft save that did not land", async ({
    page,
  }) => {
    // Ctrl+S toasted "All changes have been saved." before any write ran,
    // and a panel's Save toasted success whatever its write returned.
    test.setTimeout(150_000);
    await publishedNotebookWithCell(page, "nora", "Explicit Save");
    const notSaved = page
      .locator(".ironpad-toast-title")
      .filter({ hasText: /^Not Saved$/ });
    const saved = page.locator(".ironpad-toast-title", {
      hasText: "Notebook saved",
    });
    const metaSaved = page.locator(".ironpad-toast-title", {
      hasText: "Notebook metadata saved",
    });
    const header = page.locator(".ironpad-cell-card .ironpad-cell-header").first();

    await page.route(
      (url) => url.pathname.includes("save_mutable_draft"),
      (route) => route.abort(),
    );
    await header.click();
    await page.keyboard.press("Control+s");
    await expect(notSaved).toHaveCount(1, { timeout: 15_000 });
    await expect(saved).toHaveCount(0);
    const panel = await openMetadataPanel(page);
    const save = panel.locator("button.ironpad-btn--primary", { hasText: "Save" });
    await save.click();
    await expect(notSaved).toHaveCount(2, { timeout: 15_000 });
    await expect(metaSaved).toHaveCount(0);

    // Saves land again, and so do the success toasts.
    await page.unrouteAll();
    await header.click();
    await page.keyboard.press("Control+s");
    await expect(saved).toBeVisible({ timeout: 15_000 });
    await save.click();
    await expect(metaSaved).toBeVisible({ timeout: 15_000 });
  });

  test("leaving mid-Push while an autosave is on the wire keeps the app alive", async ({
    page,
  }) => {
    // Push's pre-promote save waits out the autosave on the wire. Leaving
    // during that wait used to read a disposed signal after it and panic the
    // whole wasm app, leaving the page navigated to dead.
    test.setTimeout(150_000);
    const disposed: string[] = [];
    page.on("console", (msg) => {
      if (msg.text().includes("already been disposed")) disposed.push(msg.text());
    });
    await publishedNotebookWithCell(page, "lena", "Leave Mid Push");
    let held = 0;
    await page.route(
      (url) => url.pathname.includes("save_mutable_draft"),
      async (route) => {
        held++;
        await new Promise((resolve) => setTimeout(resolve, 3_000));
        await route.continue().catch(() => {});
      },
    );
    await setCellSource(
      page,
      page.locator(".ironpad-cell-card").first(),
      "let left_mid_push = 1;",
    );
    await expect.poll(() => held, { timeout: 10_000 }).toBeGreaterThan(0);
    await page.locator(".ironpad-push-button").click();
    await page.waitForTimeout(400);
    await page.locator("a.ironpad-brand").click();
    await expect(page.locator(".ironpad-home")).toBeVisible({ timeout: 15_000 });
    await page.waitForTimeout(3_500);
    expect(disposed).toEqual([]);
    await page.locator("button", { hasText: "New Notebook" }).click();
    await expect(page).toHaveURL(/\/local\/[a-f0-9-]+/, { timeout: 15_000 });
  });
});
