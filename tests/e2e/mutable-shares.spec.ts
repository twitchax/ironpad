import fs from "fs";
import path from "path";

import { test, expect, APIRequestContext, Page } from "@playwright/test";
import { loginTestUser } from "./helpers/auth";
import { setCellSource } from "./helpers/monaco";
import {
  expectOwnerEditor,
  menuClick,
  shareMutable,
} from "./helpers/mutable";
import { createNotebook } from "./helpers/session";

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
 * Asserts /og/mutable/{id}.png is a real card: PNG magic bytes plus the IHDR
 * width/height, because that is what an unfurler reads to size the preview.
 */
async function expectLiveCard(
  request: APIRequestContext,
  shareId: string,
): Promise<void> {
  const res = await request.get(`${BASE}/og/mutable/${shareId}.png`);
  expect(res.status()).toBe(200);
  expect(res.headers()["content-type"]).toBe("image/png");
  const body = await res.body();
  expect(body.subarray(0, 8)).toEqual(
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
  );
  expect(body.readUInt32BE(16)).toBe(1200);
  expect(body.readUInt32BE(20)).toBe(630);
}

/** Rename via the header title input and confirm the change landed. */
async function rename(page: Page, title: string): Promise<void> {
  await page.locator(".ironpad-notebook-title--editable").click();
  const input = page.locator(".ironpad-header-title-input");
  await expect(input).toBeVisible();
  await input.fill("");
  await input.pressSequentially(title, { delay: 15 });
  await input.press("Enter");
  await expect(page.locator(".ironpad-notebook-title--editable")).toHaveText(
    title,
    { timeout: 10_000 },
  );
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
    await rename(page, "Lifecycle One");

    const shareId = await shareMutable(page);

    // Fresh share: nothing to push.
    const push = page.locator(".ironpad-push-button");
    await expect(push).toBeDisabled();
    await expect(push).toHaveText(/Published/);

    // An edit arms Push and autosaves to the DRAFT.
    await rename(page, "Lifecycle One v2");
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
    await rename(page, "Device One");
    const shareId = await shareMutable(page);

    // Draft an edit on device one and let the autosave land.
    await rename(page, "Device One WIP");
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
    await rename(page, "Discard Base");
    const shareId = await shareMutable(page);

    // Draft an edit, then throw it away.
    await rename(page, "Discard Scratch");
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

  test("unpublish brings the notebook home and 404s the link and card", async ({
    page,
    request,
  }) => {
    test.setTimeout(150_000);
    await loginTestUser(page, "alice");
    await createNotebook(page);
    await rename(page, "Unpublish Me");
    const shareId = await shareMutable(page);

    // Warm the og card so the 404-after-unpublish assertion is strict: the
    // handler must re-check existence rather than serve the cached PNG.
    await expectLiveCard(request, shareId);

    page.on("dialog", (d) => d.accept());
    await menuClick(page, "Unpublish");
    // Progress ack while it works: the flush, the local save, and the server
    // delete are several seconds of visible nothing before the navigation.
    await expect(
      page.locator(".ironpad-toast-title", { hasText: "Unpublishing" }),
    ).toBeVisible({ timeout: 10_000 });
    // Hard navigation back to /local/{uuid}; toast rides sessionStorage.
    await expect(page).toHaveURL(/\/local\/[a-f0-9-]+/, { timeout: 30_000 });
    await expect(
      page.locator(".ironpad-toast-title", { hasText: "Unpublished" }),
    ).toBeVisible({ timeout: 15_000 });
    await expect(page.locator(".ironpad-notebook-title--editable")).toHaveText(
      "Unpublish Me",
      { timeout: 15_000 },
    );

    // The share is gone server-side: hard 404 on the page AND the card.
    const gone = await request.get(`${BASE}/mutable/${shareId}`);
    expect(gone.status()).toBe(404);
    const card = await request.get(`${BASE}/og/mutable/${shareId}.png`);
    expect(card.status()).toBe(404);

    // And it's back as a private notebook on home.
    const notebookId = page.url().match(/\/local\/([a-f0-9-]+)/)![1];
    await page.goto("/");
    await expect(page.locator(".ironpad-home")).toBeVisible();
    await expect(page.locator(`a[href="/local/${notebookId}"]`)).toBeVisible({
      timeout: 10_000,
    });
  });

  test("a mutable share unfurls from the raw body with unlisted robots", async ({
    page,
    request,
  }) => {
    test.setTimeout(120_000);
    await loginTestUser(page, "alice");
    await createNotebook(page);
    const title = `Mutable unfurl ${Date.now()}`;
    await rename(page, title);
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
    await rename(page, "Push Fail Base");
    const shareId = await shareMutable(page);

    // Fault injection: every draft save fails at the network layer. The
    // debounce autosave surfaces the visible failure state (uat-006), and a
    // Push must refuse to promote — the server never received the edit, so
    // promoting would publish stale content behind a success toast.
    await page.route(
      (url) => url.pathname.includes("save_mutable_draft"),
      (route) => route.abort(),
    );
    await rename(page, "Push Fail v2");
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
    await rename(page, "Download Mutable");
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

  test("unpublish keeps a keystroke made inside the debounce window", async ({
    page,
  }) => {
    test.setTimeout(150_000);
    await loginTestUser(page, "alice");
    await createNotebook(page);
    // Rename BEFORE adding a cell: a new cell steals focus when its Monaco
    // mounts (pending_focus_cell), which would hijack mid-flight title typing.
    await rename(page, "Unpublish Flush");
    await page.locator(".ironpad-add-cell-btn").first().click();
    await expect(page.locator(".ironpad-cell-card")).toHaveCount(1);
    await shareMutable(page);
    await expect(
      page.locator(".ironpad-cell-card .monaco-editor").first(),
    ).toBeVisible({ timeout: 15_000 });

    // Edit a cell and unpublish IMMEDIATELY — inside the ~1s editor->model
    // debounce. Unpublish deletes the share (published AND draft), so the
    // local save it makes is the only surviving copy: without the flush
    // discipline this keystroke was gone permanently.
    page.on("dialog", (d) => d.accept());
    await setCellSource(
      page,
      page.locator(".ironpad-cell-card").first(),
      "let flushed_before_unpublish = 42;",
    );
    await menuClick(page, "Unpublish");

    await expect(page).toHaveURL(/\/local\/[a-f0-9-]+/, { timeout: 30_000 });
    await expect(
      page.locator(".ironpad-toast-title", { hasText: "Unpublished" }),
    ).toBeVisible({ timeout: 15_000 });
    await expect(
      page.locator(".ironpad-cell-card .monaco-editor .view-lines").first(),
    ).toContainText("flushed_before_unpublish", { timeout: 30_000 });
  });
});
