import { test, expect, APIRequestContext, Page } from "@playwright/test";
import { createNotebook } from "./helpers/session";
import { loginTestUser } from "./helpers/auth";

/**
 * PRD-0049 (accounts-backed since PRD-0053): mutable shares. Convert a
 * private notebook to a server-backed mutable share at /mutable/{id}, push
 * updates, clone-to-local on a second signed-in device, and unpublish.
 * Ownership is the GitHub session's OWNER grant; sessions here come from the
 * env-gated test login (see helpers/auth.ts).
 *
 * Also the /mutable half of the PRD-0050 unfurl contract (social-preview
 * .spec.ts owns the /public and /shared halves): the metadata assertions here
 * MUST run against raw response bodies via `request`, never the hydrated DOM.
 * Reddit, X, Slack, and Discord fetch the HTML and run no JavaScript, so tags
 * that leptos_meta patches in client-side do not exist for an unfurler.
 */

const MENU = '.ironpad-toolbar-dropdown-toggle[title="Notebook menu"]';

const BASE = "http://localhost:3111";

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
 * width/height, because that is what an unfurler reads to size the preview. A
 * 200 with a broken or wrongly-sized body would silently kill the wide card.
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

/** Open the notebook (hamburger) menu and click an item by its label. */
async function menuClick(page: Page, label: string): Promise<void> {
  await page.locator(MENU).click();
  await page
    .locator(".ironpad-toolbar-dropdown-item", { hasText: label })
    .click();
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

/** Share Mutable and return the minted share id from the toast. */
async function shareMutable(page: Page): Promise<string> {
  await menuClick(page, "Share Mutable");
  // Scope to the mutable toast: the save_generation bump also raises a
  // transient "saved" toast, so several toast bodies can be present at once.
  const toast = page.locator(".ironpad-toast-body", { hasText: "/mutable/" });
  await expect(toast).toBeVisible({ timeout: 30_000 });
  const text = (await toast.textContent())!;
  const id = text.match(/\/mutable\/([a-f0-9]{16})/);
  expect(id, `toast should carry a /mutable/{id} url: ${text}`).not.toBeNull();
  return id![1];
}

test.describe("Mutable shares (PRD-0049)", () => {
  test("convert to mutable, push an edit, and a fresh reader sees the update", async ({
    page,
    browser,
  }) => {
    test.setTimeout(90_000);
    await loginTestUser(page, "alice");
    await createNotebook(page);
    await page.waitForTimeout(1_500); // binding load (hydration)

    await rename(page, "Mutable One");
    const shareId = await shareMutable(page);

    // The menu swaps Share Mutable → Push Update once bound.
    await page.locator(MENU).click();
    await expect(
      page.locator(".ironpad-toolbar-dropdown-item", {
        hasText: "Push Update",
      }),
    ).toBeVisible();
    await expect(
      page.locator(".ironpad-toolbar-dropdown-item", {
        hasText: "Share Mutable",
      }),
    ).toHaveCount(0);
    await page.locator(MENU).click(); // close

    // Edit, then Push the update. The immediate "Pushing…" ack shows while
    // the snapshot round-trips, then the success toast replaces it.
    await rename(page, "Mutable One Edited");
    await menuClick(page, "Push Update");
    await expect(
      page.locator(".ironpad-toast-title", { hasText: "Pushing" }),
    ).toBeVisible();
    await expect(
      page.locator(".ironpad-toast-body", { hasText: "updated" }),
    ).toBeVisible({ timeout: 30_000 });

    // A fresh context (no shared IndexedDB) reads the server copy.
    const ctx = await browser.newContext();
    try {
      const reader = await ctx.newPage();
      await reader.goto(`/mutable/${shareId}`);
      await expect(reader.locator(".view-only-notebook")).toBeVisible({
        timeout: 30_000,
      });
      await expect(reader.locator(".view-only-title")).toHaveText(
        "Mutable One Edited",
        { timeout: 15_000 },
      );
      // Owner attribution renders for everyone (PRD-0053 uat-004).
      await expect(reader.locator(".mutable-attribution")).toContainText(
        "@alice",
      );
    } finally {
      await ctx.close();
    }
  });

  test("a mutable share unfurls from the raw body and a push updates the unfurl", async ({
    page,
    request,
  }) => {
    // Two server round-trips that snapshot blobs (create + push), so the
    // suite-standard generous budget.
    test.setTimeout(120_000);
    await loginTestUser(page, "alice");
    await createNotebook(page);
    await page.waitForTimeout(1_500); // binding load (hydration)

    // Unique title so a stale server-side record from an earlier run can
    // never satisfy the assertion by accident.
    const title = `Mutable unfurl ${Date.now()}`;
    await rename(page, title);
    // shareMutable enforces the minted id's 16-hex shape (PRD-0049 contract).
    const shareId = await shareMutable(page);

    const html = await rawHtml(request, `/mutable/${shareId}`);
    // The regression this guards: /mutable renders its metadata from an async
    // Resource, and under the default streaming SSR the head is flushed
    // before it resolves — the tags then look right in devtools and are
    // invisible to every unfurler. SsrMode::Async on this route is
    // load-bearing, and only the raw body can prove it.
    expect(metaContent(html, "og:title")).toBe(title);
    // Absolute, because a crawler has no document base to resolve against.
    expect(metaContent(html, "og:url")).toBe(`${BASE}/mutable/${shareId}`);
    expect(metaContent(html, "og:image")).toBe(
      `${BASE}/og/mutable/${shareId}.png`,
    );
    // Unlisted, not secret (PRD-0050 uat-005 for the mutable class): noindex
    // on the page rather than a robots.txt Disallow, because several
    // unfurlers honour robots.txt and would refuse to build a preview at all.
    expect(metaContent(html, "robots")).toContain("noindex");

    // The advertised card must actually exist at 1200x630 (uat-002).
    await expectLiveCard(request, shareId);

    // Push an edit. The card URL is stable across edits by design, so the
    // unfurl content must track the server copy: a pasted link should preview
    // what the author last pushed, not what they first shared.
    const updated = `${title} v2`;
    await rename(page, updated);
    await menuClick(page, "Push Update");
    await expect(
      page.locator(".ironpad-toast-body", { hasText: "updated" }),
    ).toBeVisible({ timeout: 30_000 });

    const pushed = await rawHtml(request, `/mutable/${shareId}`);
    expect(metaContent(pushed, "og:title")).toBe(updated);
  });

  test("unpublish returns the notebook to the private list and 404s the link", async ({
    page,
    request,
  }) => {
    test.setTimeout(90_000);
    await loginTestUser(page, "alice");
    await createNotebook(page);
    await page.waitForTimeout(1_500);
    const notebookId = page.url().match(/\/local\/([a-f0-9-]+)/)![1];

    const shareId = await shareMutable(page);

    // Render the card while the share is live. This also warms the og disk
    // cache, which makes the 404-after-unpublish assertion below strict: the
    // handler must re-check the share's existence rather than serve the
    // cached PNG.
    await expectLiveCard(request, shareId);

    // Delete is replaced by Unpublish while mutable-backed.
    await page.locator(MENU).click();
    await expect(
      page.locator(".ironpad-toolbar-dropdown-item", { hasText: "Unpublish" }),
    ).toBeVisible();
    page.on("dialog", (d) => d.accept());
    await page
      .locator(".ironpad-toolbar-dropdown-item", { hasText: "Unpublish" })
      .click();
    await expect(
      page.locator(".ironpad-toast-body", { hasText: "private list" }),
    ).toBeVisible({ timeout: 30_000 });

    // The share is gone server-side, and the resolve is a hard HTTP 404
    // (mark_not_found sets the status), not a 200 error shell: crawlers and
    // unfurlers drop the link instead of caching a "not found" preview.
    const gone = await request.get(`${BASE}/mutable/${shareId}`);
    expect(gone.status()).toBe(404);

    // The card 404s with it — despite the warmed disk cache above. Otherwise
    // a notebook the author explicitly took down would keep unfurling.
    const card = await request.get(`${BASE}/og/mutable/${shareId}.png`);
    expect(card.status()).toBe(404);

    // And the reader page tells a human why.
    await page.goto(`/mutable/${shareId}`);
    await expect(page.locator(".ironpad-error-boundary-message")).toContainText(
      "not found",
      { timeout: 15_000 },
    );

    // And it's back as a private notebook on home.
    await page.goto("/");
    await expect(page.locator(".ironpad-home")).toBeVisible();
    await expect(page.locator(`a[href="/local/${notebookId}"]`)).toBeVisible({
      timeout: 10_000,
    });
  });

  test("author round-trip: edit shortcut, divergence banner, pull, view published", async ({
    page,
  }) => {
    test.setTimeout(120_000);
    await loginTestUser(page, "alice");
    await createNotebook(page);
    await page.waitForTimeout(1_500); // binding load (hydration)

    const title = `Round trip ${Date.now()}`;
    await rename(page, title);
    const shareId = await shareMutable(page);

    // The published URL is findable after its one appearance in the share
    // toast: the metadata panel shows it with a copy control.
    await page
      .locator(".view-only-shared-header", { hasText: "Notebook Metadata" })
      .click();
    await expect(
      page.locator(".ironpad-metadata-published-url code"),
    ).toContainText(`/mutable/${shareId}`);

    // View Published closes the loop from the editor to the reader page.
    await menuClick(page, "View Published");
    await expect(page).toHaveURL(new RegExp(`/mutable/${shareId}`));

    // The authoring device gets a first-class Edit shortcut, and no banner:
    // the working copy matches what was just published. (The edit button
    // appearing proves the binding check resolved, which makes the banner
    // absence assertion strict rather than a race.)
    const edit = page.locator(".view-only-edit-button");
    await expect(edit).toBeVisible({ timeout: 15_000 });
    await expect(page.locator(".mutable-author-banner")).toHaveCount(0);

    // The menu offers the editor on the authoring device.
    const readerMenuToggle = page.locator(
      ".view-only-menu .ironpad-toolbar-dropdown-toggle",
    );
    await readerMenuToggle.click();
    await expect(page.locator(".mutable-edit-menu-item")).toBeVisible();
    await readerMenuToggle.click(); // close

    // Edit drops straight into the editor; no key required.
    await edit.click();
    await expect(page).toHaveURL(/\/local\/[a-f0-9-]+/);
    await expect(page.locator(".ironpad-editor")).toBeVisible({
      timeout: 15_000,
    });

    // An unpushed edit: the reader keeps showing the published title (the
    // reader renders the server copy, not the local one) and raises the
    // divergence banner for the author.
    const draft = `${title} draft`;
    await rename(page, draft);
    await page.waitForTimeout(1_500); // let the IndexedDB save land
    await page.goto(`/mutable/${shareId}`);
    await expect(page.locator(".view-only-title")).toHaveText(title, {
      timeout: 30_000,
    });
    await expect(page.locator(".mutable-author-banner")).toBeVisible({
      timeout: 15_000,
    });

    // The banner's editor link works.
    await page.locator(".mutable-author-banner-link").click();
    await expect(page.locator(".ironpad-editor")).toBeVisible({
      timeout: 15_000,
    });

    // Pull Latest discards the local draft in favor of the published copy;
    // it confirms first and ends in a full reload. The success toast rides
    // sessionStorage across the reload (a live toast dies with the page).
    await page.waitForTimeout(1_000); // binding load
    page.on("dialog", (d) => d.accept());
    await menuClick(page, "Pull Latest");
    await expect(
      page.locator(".ironpad-toast-title", { hasText: "Pulled" }),
    ).toBeVisible({ timeout: 30_000 });
    await expect(page.locator(".ironpad-notebook-title--editable")).toHaveText(
      title,
      { timeout: 30_000 },
    );

    // Local and published agree again, so the banner is gone. Gate on the
    // edit shortcut (binding resolved) before asserting absence.
    await page.goto(`/mutable/${shareId}`);
    await expect(page.locator(".view-only-edit-button")).toBeVisible({
      timeout: 15_000,
    });
    await expect(page.locator(".mutable-author-banner")).toHaveCount(0);

    // Pulling again is a no-op and says so; the destructive confirm never
    // fires for matching copies (the accept-all dialog handler above would
    // reload if it did, and the Up to Date toast would never appear).
    await page.locator(".view-only-edit-button").click();
    await expect(page.locator(".ironpad-editor")).toBeVisible({
      timeout: 15_000,
    });
    await page.waitForTimeout(1_000); // binding load
    await menuClick(page, "Pull Latest");
    await expect(
      page.locator(".ironpad-toast-title", { hasText: "Up to Date" }),
    ).toBeVisible({ timeout: 15_000 });
  });

  test("second device: the owner clones to local and pushes; others get no edit controls", async ({
    page,
    browser,
  }) => {
    test.setTimeout(150_000);
    await loginTestUser(page, "alice");
    await createNotebook(page);
    await page.waitForTimeout(1_500);

    const title = `Second device ${Date.now()}`;
    await rename(page, title);
    const shareId = await shareMutable(page);

    // A fresh context signed in as the SAME account: Edit clones the
    // published copy into a local working copy and opens the editor — the
    // PRD-0053 replacement for the key-based rebind (signing in IS the
    // authorization; distinct test logins are distinct accounts).
    const sameOwner = await browser.newContext();
    try {
      const p2 = await sameOwner.newPage();
      await loginTestUser(p2, "alice");
      await p2.goto(`/mutable/${shareId}`);
      await expect(p2.locator(".view-only-notebook")).toBeVisible({
        timeout: 30_000,
      });
      await expect(p2.locator(".mutable-attribution")).toContainText("@alice");

      const edit = p2.locator(".view-only-edit-button");
      await expect(edit).toBeVisible({ timeout: 15_000 });
      await edit.click();
      // The clone is confirmed with a toast (asserted before the editor —
      // it auto-dismisses), then lands in the editor.
      await expect(
        p2.locator(".ironpad-toast-title", { hasText: "Ready to Edit" }),
      ).toBeVisible({ timeout: 15_000 });
      await expect(p2).toHaveURL(/\/local\/[a-f0-9-]+/, { timeout: 15_000 });
      await expect(p2.locator(".ironpad-editor")).toBeVisible({
        timeout: 15_000,
      });

      // Push works from the cloned device: same account, same OWNER grant.
      await p2.waitForTimeout(1_000); // binding load
      await menuClick(p2, "Push Update");
      await expect(
        p2.locator(".ironpad-toast-body", { hasText: "updated" }),
      ).toBeVisible({ timeout: 30_000 });
    } finally {
      await sameOwner.close();
    }

    // A DIFFERENT signed-in account reads the notebook but gets no edit
    // surface at all — ownership is per-account, not per-login-state.
    const otherUser = await browser.newContext();
    try {
      const p3 = await otherUser.newPage();
      await loginTestUser(p3, "bob");
      await p3.goto(`/mutable/${shareId}`);
      await expect(p3.locator(".view-only-notebook")).toBeVisible({
        timeout: 30_000,
      });
      await p3.waitForTimeout(2_000); // let any owner UI hydrate (it must not)
      await expect(p3.locator(".view-only-edit-button")).toHaveCount(0);
      await expect(p3.locator(".mutable-edit-menu-item")).toHaveCount(0);
    } finally {
      await otherUser.close();
    }

    // Anonymous readers: same read-only page, attribution included.
    const anon = await browser.newContext();
    try {
      const p4 = await anon.newPage();
      await p4.goto(`/mutable/${shareId}`);
      await expect(p4.locator(".view-only-notebook")).toBeVisible({
        timeout: 30_000,
      });
      await expect(p4.locator(".mutable-attribution")).toContainText("@alice");
      await p4.waitForTimeout(2_000);
      await expect(p4.locator(".view-only-edit-button")).toHaveCount(0);
    } finally {
      await anon.close();
    }
  });
});
