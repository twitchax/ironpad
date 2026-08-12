import fs from "fs";
import path from "path";

import { test, expect, APIRequestContext } from "@playwright/test";
import { loginTestUser } from "./helpers/auth";
import { setCellSource } from "./helpers/monaco";
import {
  expectLiveCard,
  expectOwnerEditor,
  menuClick,
  shareMutable,
} from "./helpers/mutable";
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
});
