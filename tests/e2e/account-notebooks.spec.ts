import { test, expect, Page } from "@playwright/test";

import { loginTestUser } from "./helpers/auth";
import { openMetadataPanel } from "./helpers/metadata";
import { setCellSource } from "./helpers/monaco";
import {
  MENU,
  expectLiveCard,
  expectOwnerEditor,
  menuClick,
  saveToAccount,
} from "./helpers/mutable";
import { ADD_CODE, createNotebook, renameNotebook } from "./helpers/session";

/**
 * PRD-0064: Save to Account — server-stored private notebooks, with
 * publishing as a flag on the same row rather than a second storage class.
 *
 * An account notebook IS a `mutable_share` whose published copy is absent,
 * so the whole lifecycle happens at ONE address: /mutable/{id} is the
 * owner's editor from the moment it is saved, 404 to everyone else until it
 * is published, and 404 again after it is unpublished — with the URL never
 * moving. That invariant is the point of the spec: every step below
 * re-asserts the same id.
 *
 * Anonymous surfaces are asserted against RAW response status and bodies via
 * `request`, never the hydrated DOM. PRD-0050 and PRD-0063 both shipped a
 * status race that renders correctly and reports 200.
 */

const BASE = "http://localhost:3111";

/** A marker only this notebook's cell carries, so "content survived" is exact. */
const CELL_MARKER = "let saved_to_account = 64;";

/**
 * Reader copy, quoted rather than paraphrased. Each of these is a literal in
 * `pages/mutable_notebook.rs` or `notebook_editor/metadata_panel.rs`, and
 * each one is the ONLY on-screen difference between two states that share
 * one address — so a drifted string here is a real regression, not churn.
 */
const NOT_FOUND_COPY = "This mutable notebook was not found.";
const LOADING_COPY = "Loading notebook";
const UNPUBLISHED_URL_LABEL = "Notebook link (not published)";
const UNPUBLISHED_URL_HINT =
  "Only you can open this. Publish to make the link work for anyone else.";
const PUBLISHED_URL_LABEL = "Published at";

/**
 * Record whether the reader's not-found copy is EVER in the DOM, from the
 * first parsed node onward, for every navigation this page makes afterwards.
 *
 * A `toHaveCount(0)` cannot make this claim: the bug it guards is a FLASH,
 * and a poll that arrives after the ownership probe resolves sees a clean
 * DOM and passes. The observer runs before any page script and watches the
 * document itself, so it sees SSR markup arriving from the parser as well as
 * anything hydration swaps in.
 */
async function watchForNotFoundCopy(page: Page): Promise<void> {
  await page.addInitScript((needle: string) => {
    const w = window as unknown as Record<string, unknown>;
    w.__ironpadSawNotFound = false;
    const scan = () => {
      if (document.body?.textContent?.includes(needle)) {
        w.__ironpadSawNotFound = true;
      }
    };
    // `document`, not `documentElement`: at document-start the root element
    // may not exist yet, and observing the document covers it once it does.
    new MutationObserver(scan).observe(document, {
      childList: true,
      subtree: true,
      characterData: true,
    });
    scan();
  }, NOT_FOUND_COPY);
}

/** Whether the not-found copy has appeared since the last navigation. */
function sawNotFoundCopy(page: Page): Promise<boolean> {
  return page.evaluate(
    () => (window as unknown as Record<string, unknown>).__ironpadSawNotFound,
  ) as Promise<boolean>;
}

/**
 * Clear the browser's notebook store outright, then reload.
 *
 * uat-001 asks for the content to survive a reload "with the browser store
 * cleared": Save to Account deletes only the one record, so a reload alone
 * leaves open the possibility that some other local cache carried the
 * content. Dropping the whole database removes that reading.
 */
async function clearLocalStoreAndReload(page: Page): Promise<void> {
  await page.evaluate(
    () =>
      new Promise<void>((resolve) => {
        const req = indexedDB.deleteDatabase("ironpad");
        req.onsuccess = () => resolve();
        req.onerror = () => resolve();
        // A live connection blocks the delete; storage.js closes after every
        // operation, but a resolve here keeps a straggler from hanging the
        // test rather than failing it for the wrong reason.
        req.onblocked = () => resolve();
      }),
  );
  await page.reload();
}

/** The stored record for a local notebook id, or null once it is gone. */
function localRecord(page: Page, uuid: string): Promise<unknown> {
  return page.evaluate(
    (id) => (window as any).IronpadStorage.getNotebook(id),
    uuid,
  );
}

/** The reader's not-found panel — what a stranger sees at an unpublished id. */
async function expectNotFoundPanel(page: Page): Promise<void> {
  await expect(
    page.locator(".ironpad-error-boundary-message", {
      hasText: NOT_FOUND_COPY,
    }),
  ).toBeVisible({ timeout: 30_000 });
}

test.describe("Account notebooks (PRD-0064)", () => {
  test("save, reload, publish, unpublish, delete: one URL throughout", async ({
    page,
    browser,
    request,
  }) => {
    test.setTimeout(180_000);
    await loginTestUser(page, "erin");
    await createNotebook(page);
    // Rename BEFORE adding a cell: a new cell steals focus when its Monaco
    // mounts (pending_focus_cell), which would hijack mid-flight typing.
    await renameNotebook(page, "Account Lifecycle");
    await page.locator(ADD_CODE).first().click();
    await expect(page.locator(".ironpad-cell-card")).toHaveCount(1);
    // Monaco's global arrives with its own script, after the card mounts;
    // setCellSource reaches for `window.monaco` and throws if it is early.
    await expect(
      page.locator(".ironpad-cell-card .monaco-editor").first(),
    ).toBeVisible({ timeout: 15_000 });
    await setCellSource(
      page,
      page.locator(".ironpad-cell-card").first(),
      CELL_MARKER,
    );
    const localUuid = page.url().match(/\/local\/([a-f0-9-]+)/)![1];

    // ── Save to Account: move, never copy ───────────────────────────────
    const shareId = await saveToAccount(page);
    // The local record is GONE. Two unreconciled copies of one notebook is
    // the failure PRD-0054 removed, and a feature whose point is durable
    // storage must not reintroduce it as a convenience.
    expect(await localRecord(page, localUuid)).toBeNull();
    await expect(page).toHaveURL(new RegExp(`/mutable/${shareId}$`));

    // Home lists it as an account notebook, badged unpublished (the lock,
    // with no "published" hint), and the local card is gone with the record.
    await page.goto("/");
    await expect(page.locator(".ironpad-home")).toBeVisible({
      timeout: 15_000,
    });
    const card = page.locator(
      `.ironpad-notebook-card-wrapper:has(a[href="/mutable/${shareId}"])`,
    );
    await expect(card).toBeVisible({ timeout: 15_000 });
    await expect(
      card.locator(".ironpad-notebook-badge.mutable.unpublished"),
    ).toBeVisible();
    await expect(card.locator(".ironpad-notebook-card-mutable-hint")).toHaveCount(
      0,
    );
    await expect(page.locator(`a[href="/local/${localUuid}"]`)).toHaveCount(0);

    // ── The content lives on the server ─────────────────────────────────
    await page.goto(`/mutable/${shareId}`);
    await expectOwnerEditor(page);
    await clearLocalStoreAndReload(page);
    await expectOwnerEditor(page);
    await expect(page.locator(".ironpad-notebook-title--editable")).toHaveText(
      "Account Lifecycle",
      { timeout: 15_000 },
    );
    await expect(
      page.locator(".ironpad-cell-card .monaco-editor .view-lines").first(),
    ).toContainText("saved_to_account", { timeout: 30_000 });

    // Unpublished: the one button offers Publish, armed, because an account
    // notebook is permanently dirty by construction (its content IS the
    // draft) — "Push" would be a lie about a notebook nobody can read.
    // Exact text, not /Publish/: "Published" matches that too, so the regex
    // cannot tell the armed first-publish state from the disabled resting one.
    const publish = page.locator(".ironpad-push-button");
    await expect(publish).toHaveText("Publish", { timeout: 15_000 });
    await expect(publish).toBeEnabled();
    expect((await request.get(`${BASE}/mutable/${shareId}`)).status()).toBe(404);

    // ── Publish ─────────────────────────────────────────────────────────
    await publish.click();
    await expect(
      page.locator(".ironpad-toast-title", { hasText: "Published" }),
    ).toBeVisible({ timeout: 60_000 });
    await expect(publish).toBeDisabled({ timeout: 15_000 });
    await expect(publish).toHaveText("Published");

    // Warm the OG card so the 404 after Unpublish is a strict assertion: the
    // card is cached server-side, so a 404 only proves the handler re-checks
    // existence if a rendered PNG was stored first.
    await expectLiveCard(request, shareId);

    // An anonymous visitor reads it — from a fresh context, so no session
    // cookie can be mistaken for the reason it resolves.
    const anon = await browser.newContext();
    try {
      const reader = await anon.newPage();
      await reader.goto(`/mutable/${shareId}`);
      await expect(reader.locator(".view-only-title")).toHaveText(
        "Account Lifecycle",
        { timeout: 30_000 },
      );
      expect(
        (await anon.request.get(`${BASE}/mutable/${shareId}`)).status(),
      ).toBe(200);
    } finally {
      await anon.close();
    }

    // ── Unpublish, in place ─────────────────────────────────────────────
    // No navigation, no IndexedDB write, no "this is your only copy" confirm:
    // the published copy is cleared and the notebook stays where it is.
    page.on("dialog", (d) => d.accept());
    await menuClick(page, "Unpublish");
    await expect(
      page.locator(".ironpad-toast-title", { hasText: "Unpublished" }),
    ).toBeVisible({ timeout: 30_000 });
    // Same URL, same editor, still holding the same content.
    await expect(page).toHaveURL(new RegExp(`/mutable/${shareId}$`));
    await expectOwnerEditor(page);
    await expect(publish).toHaveText("Publish", { timeout: 15_000 });
    await expect(publish).toBeEnabled();
    await expect(
      page.locator(".ironpad-cell-card .monaco-editor .view-lines").first(),
    ).toContainText("saved_to_account");

    // Readers are shut out again, on the page and on the card.
    expect((await request.get(`${BASE}/mutable/${shareId}`)).status()).toBe(404);
    expect(
      (await request.get(`${BASE}/og/mutable/${shareId}.png`)).status(),
    ).toBe(404);

    // ── Delete ──────────────────────────────────────────────────────────
    // Unpublish removes nothing now, so an account notebook needs its own
    // Delete — the same act Local mode's Delete performs on the record.
    await menuClick(page, "Delete");
    await expect(page).toHaveURL(`${BASE}/`, { timeout: 30_000 });
    await expect(
      page.locator(".ironpad-toast-title", { hasText: "Deleted" }),
    ).toBeVisible({ timeout: 15_000 });
    await expect(page.locator(`a[href="/mutable/${shareId}"]`)).toHaveCount(0);
    expect((await request.get(`${BASE}/mutable/${shareId}`)).status()).toBe(404);
  });

  test("an unpublished notebook is not-found for anonymous AND for another account", async ({
    page,
    browser,
    request,
  }) => {
    test.setTimeout(180_000);

    // A second signed-in identity, minted before the notebook exists so the
    // denial cannot be confused with "that user has never been seen".
    const strangerCtx = await browser.newContext();
    const stranger = await strangerCtx.newPage();
    await loginTestUser(stranger, "grace");

    await loginTestUser(page, "frank");
    await createNotebook(page);
    const title = `Unpublished Secret ${Date.now()}`;
    await renameNotebook(page, title);
    const shareId = await saveToAccount(page);

    try {
      // Anonymous: 404 on the raw response, the not-found panel in the DOM,
      // and no title anywhere in the body an unfurler would scrape.
      const anon = await browser.newContext();
      try {
        const anonPage = await anon.newPage();
        await anonPage.goto(`/mutable/${shareId}`);
        await expectNotFoundPanel(anonPage);
        await expect(anonPage.locator(".ironpad-push-button")).toHaveCount(0);

        const raw = await anon.request.get(`${BASE}/mutable/${shareId}`);
        expect(raw.status()).toBe(404);
        expect(await raw.text()).not.toContain(title);

        // The anonymous surfaces refuse rather than leak: no card, no oEmbed.
        expect(
          (await anon.request.get(`${BASE}/og/mutable/${shareId}.png`)).status(),
        ).toBe(404);
        expect(
          (
            await anon.request.get(
              `${BASE}/oembed?url=${encodeURIComponent(
                `${BASE}/mutable/${shareId}`,
              )}`,
            )
          ).status(),
        ).toBe(404);
      } finally {
        await anon.close();
      }

      // A DIFFERENT signed-in user is denied identically. Having an account
      // is not access: the OWNER grant is, and only its holder gets the
      // editor swap on hydrate.
      await stranger.goto(`/mutable/${shareId}`);
      await expectNotFoundPanel(stranger);
      await stranger.waitForTimeout(2_000); // the swap must NOT happen
      await expect(stranger.locator(".ironpad-push-button")).toHaveCount(0);
      const strangerRaw = await strangerCtx.request.get(
        `${BASE}/mutable/${shareId}`,
      );
      expect(strangerRaw.status()).toBe(404);
      expect(await strangerRaw.text()).not.toContain(title);

      // Nor is it listed for them: an account listing is by OWNER grant.
      await stranger.goto("/");
      await expect(stranger.locator(".ironpad-home")).toBeVisible({
        timeout: 15_000,
      });
      await expect(
        stranger.locator(`a[href="/mutable/${shareId}"]`),
      ).toHaveCount(0);

      // Positive control: the owner still has it, so the four denials above
      // are about access rather than about a notebook that failed to save.
      await page.goto(`/mutable/${shareId}`);
      await expectOwnerEditor(page);
      await expect(page.locator(".ironpad-notebook-title--editable")).toHaveText(
        title,
        { timeout: 15_000 },
      );
      // The sitemap must not enumerate it either (also asserted server-side,
      // in crates/ironpad-server/tests/unpublished_notebooks.rs).
      const sitemap = await request.get(`${BASE}/sitemap.xml`);
      expect(await sitemap.text()).not.toContain(`/mutable/${shareId}`);
    } finally {
      await strangerCtx.close();
    }
  });

  test("Save to Account is a signed-in affordance only", async ({ page }) => {
    test.setTimeout(120_000);
    await createNotebook(page);
    const localUrl = page.url();

    // Anonymous, with auth RESOLVED first: `signed_in` is false both before
    // get_auth_info answers and after it answers "nobody", so asserting the
    // item's absence without waiting would pass on a page that had not
    // decided yet. The sign-in control is the resolved-anonymous marker.
    await expect(page.locator(".ironpad-auth-signin")).toBeVisible({
      timeout: 15_000,
    });
    await page.locator(MENU).click();
    const items = page.locator(".ironpad-toolbar-dropdown-item");
    await expect(items.filter({ hasText: "Share Immutable" })).toBeVisible();
    await expect(items.filter({ hasText: "Save to Account" })).toHaveCount(0);

    // Signing in makes it appear on the SAME notebook: the item is gated on
    // the account, not on anything about the notebook.
    await loginTestUser(page, "heidi");
    await page.goto(localUrl);
    await expect(page.locator(".ironpad-editor")).toBeVisible({
      timeout: 15_000,
    });
    await page.locator(MENU).click();
    await expect(
      items.filter({ hasText: "Save to Account" }),
    ).toBeVisible({ timeout: 15_000 });
  });

  test("the metadata panel gates Access on publish and never claims an unpublished notebook is published", async ({
    page,
  }) => {
    // Two states, ONE address (PRD-0064: the URL does not change when you
    // publish), so everything that distinguishes them is copy and gating.
    // The Access section is PRD-0061's access UI being conditionally
    // removed: a private toggle over a notebook nobody can reach offers to
    // change nothing, and it shipped with no test at all.
    test.setTimeout(150_000);
    await loginTestUser(page, "ivy");
    await createNotebook(page);
    await renameNotebook(page, "Access Gating");
    const shareId = await saveToAccount(page);

    const section = await openMetadataPanel(page);

    // ── Unpublished ─────────────────────────────────────────────────────
    await expect(
      section.locator(".ironpad-metadata-label", {
        hasText: UNPUBLISHED_URL_LABEL,
      }),
    ).toBeVisible();
    await expect(
      section.locator(".ironpad-metadata-hint", {
        hasText: UNPUBLISHED_URL_HINT,
      }),
    ).toBeVisible();
    // Not "the label is right" but "the false claim is nowhere in the
    // panel": the copy button sits directly under this row, and inviting
    // the owner to hand out a link that 404s for the recipient is the whole
    // failure. Substring over the panel's text, since a second row asserting
    // it would pass the label check above.
    expect(await section.textContent()).not.toContain(PUBLISHED_URL_LABEL);
    // The address itself is still shown and still copyable — the owner's own
    // link does work, which is why this is relabelled rather than hidden.
    await expect(
      section.locator(".ironpad-metadata-published-url code"),
    ).toHaveText(`${BASE}/mutable/${shareId}`);

    // PRD-0061's Access UI is absent, asserted on the parts the section
    // renders UNCONDITIONALLY. (`.ironpad-access-add` is nested under the
    // private toggle, so a count of 0 for it would hold on a published
    // notebook too and would prove nothing here.)
    await expect(
      section.locator(".ironpad-metadata-label", { hasText: "Access" }),
    ).toHaveCount(0);
    await expect(section.locator(".ironpad-access-toggle")).toHaveCount(0);

    // The draft-side menu items are gated on the same flag. Discard Draft in
    // particular MUST stay away: an account notebook's draft is its only
    // content, and discarding it now answers Ok(false) rather than reloading
    // over untouched content.
    await page.locator(MENU).click();
    const items = page.locator(".ironpad-toolbar-dropdown-item");
    await expect(items.filter({ hasText: "Discard Draft" })).toHaveCount(0);
    await expect(items.filter({ hasText: "View Published" })).toHaveCount(0);
    // History is a LOCAL-notebook feature and does not follow a notebook to
    // the server, which is exactly what the Save to Account confirm warns
    // about; assert the loss rather than only the warning.
    await expect(items.filter({ hasText: "History" })).toHaveCount(0);
    await page.keyboard.press("Escape");

    // ── Publish ─────────────────────────────────────────────────────────
    const publish = page.locator(".ironpad-push-button");
    await expect(publish).toHaveText("Publish", { timeout: 15_000 });
    await publish.click();
    await expect(
      page.locator(".ironpad-toast-title", { hasText: "Published" }),
    ).toBeVisible({ timeout: 60_000 });
    await expect(publish).toBeDisabled({ timeout: 15_000 });

    // Same panel, same address row, no reload: the label is the only
    // observable difference, so it has to actually move.
    await expect(
      section.locator(".ironpad-metadata-label", {
        hasText: PUBLISHED_URL_LABEL,
      }),
    ).toBeVisible({ timeout: 15_000 });
    await expect(
      section.locator(".ironpad-metadata-hint", {
        hasText: UNPUBLISHED_URL_HINT,
      }),
    ).toHaveCount(0);
    expect(await section.textContent()).not.toContain(UNPUBLISHED_URL_LABEL);

    // Access arrives with publication, wired to the real server fn: the
    // toggle enables only once get_mutable_access has answered.
    await expect(
      section.locator(".ironpad-metadata-label", { hasText: "Access" }),
    ).toBeVisible({ timeout: 15_000 });
    await expect(section.locator(".ironpad-access-toggle")).toBeVisible();
    await expect(section.locator(".ironpad-access-toggle input")).toBeEnabled({
      timeout: 15_000,
    });

    // And the READ-grant half with it. It lives behind the private toggle,
    // so reaching it is the only way to prove the whole PRD-0061 section
    // came back rather than just its header.
    await section.locator(".ironpad-access-toggle input").check();
    await expect(
      page.locator(".ironpad-toast-title", { hasText: "private" }),
    ).toBeVisible({ timeout: 15_000 });
    await expect(section.locator(".ironpad-access-add input")).toBeVisible();
  });

  test("the owner's own unpublished notebook never paints not-found copy", async ({
    page,
    browser,
  }) => {
    // SSR always renders the reader, and an unpublished account notebook is
    // a 404 on the reader path — including to its owner, whose ownership is
    // only resolved after hydrate. Without the neutral placeholder, the
    // PRIMARY flow of this PRD first-paints the owner's own notebook as
    // missing, with a warning icon, on every single load.
    test.setTimeout(180_000);
    await loginTestUser(page, "judy");
    await createNotebook(page);
    const title = `Owner Placeholder ${Date.now()}`;
    await renameNotebook(page, title);

    // Installed BEFORE the save, so it covers the hard navigation Save to
    // Account performs — the first load of the notebook's new address.
    await watchForNotFoundCopy(page);
    const shareId = await saveToAccount(page);
    expect(await sawNotFoundCopy(page)).toBe(false);

    // A fresh hard load of the same URL, which is the everyday case.
    await page.goto(`/mutable/${shareId}`);
    await expectOwnerEditor(page);
    expect(await sawNotFoundCopy(page)).toBe(false);

    // The raw SSR body, not the hydrated DOM: PRD-0050 and PRD-0063 both
    // shipped divergences here that render correctly and report the wrong
    // thing, and the first paint is precisely what is under test.
    const owner = await page.context().request.get(`${BASE}/mutable/${shareId}`);
    // Still 404. The placeholder chooses COPY and nothing else — an owner's
    // unpublished notebook must stay unreal to crawlers and unfurlers.
    expect(owner.status()).toBe(404);
    const ownerBody = await owner.text();
    expect(ownerBody).toContain(LOADING_COPY);
    expect(ownerBody).not.toContain(NOT_FOUND_COPY);

    // An anonymous visitor at the SAME id is byte-for-byte the old behavior:
    // the not-found panel, never the placeholder. Without this half, a
    // change that showed everyone the placeholder would pass above.
    const anon = await browser.newContext();
    try {
      const anonRes = await anon.request.get(`${BASE}/mutable/${shareId}`);
      expect(anonRes.status()).toBe(404);
      const anonBody = await anonRes.text();
      expect(anonBody).toContain(NOT_FOUND_COPY);
      expect(anonBody).not.toContain(LOADING_COPY);

      const anonPage = await anon.newPage();
      await anonPage.goto(`/mutable/${shareId}`);
      await expectNotFoundPanel(anonPage);
      // And it STAYS: no flicker into the placeholder once hydration runs.
      await anonPage.waitForTimeout(2_000);
      await expectNotFoundPanel(anonPage);
    } finally {
      await anon.close();
    }

    // A soft navigation (client-side route change, no SSR response at all)
    // is the LayoutContext::auth arm of the predicate, and no raw-body
    // assertion can reach it.
    await page.goto("/");
    await expect(page.locator(".ironpad-home")).toBeVisible({
      timeout: 15_000,
    });
    await page.waitForTimeout(3_000); // hydration (suite convention)
    expect(await sawNotFoundCopy(page)).toBe(false);
    await page.locator(`a[href="/mutable/${shareId}"]`).first().click();
    await expectOwnerEditor(page);
    expect(await sawNotFoundCopy(page)).toBe(false);

    // `?view=reader` skips the ownership probe entirely, so the placeholder
    // has nothing to wait for: the real answer must land immediately rather
    // than leaving the owner on a spinner forever.
    await page.goto(`/mutable/${shareId}?view=reader`);
    await expectNotFoundPanel(page);
  });

  test("Save to Account names what it destroys, and dismissing it changes nothing", async ({
    page,
  }) => {
    // The act deletes the browser-local copy AND its version history ring,
    // neither of which is recoverable, so the confirm is the fix and its
    // TEXT is the whole of it. A spec that blindly accepts proves nothing.
    test.setTimeout(150_000);
    await loginTestUser(page, "kyle");
    await createNotebook(page);
    await renameNotebook(page, "Confirm Gate");
    const localUuid = page.url().match(/\/local\/([a-f0-9-]+)/)![1];

    // ── Dismiss ─────────────────────────────────────────────────────────
    // Only the negative direction catches a decorative confirm: an
    // implementation that ignores the answer passes every accept-path
    // assertion in this file.
    let dismissed = "";
    page.once("dialog", (d) => {
      dismissed = d.message();
      void d.dismiss();
    });
    await menuClick(page, "Save to Account");
    expect(dismissed).toContain("version history");
    expect(dismissed).toContain("deleted");
    expect(dismissed.toLowerCase()).toContain("publish");

    // Nothing happened: no upload, no navigation, no local delete. The
    // progress toast is emitted after the confirm, so its absence is the
    // proof that the flow never started rather than that it was fast.
    await page.waitForTimeout(2_000);
    expect(page.url()).toContain(`/local/${localUuid}`);
    expect(await localRecord(page, localUuid)).not.toBeNull();
    await expect(
      page.locator(".ironpad-toast-title", { hasText: "Saving" }),
    ).toHaveCount(0);

    // ── Accept ──────────────────────────────────────────────────────────
    // Same copy, the other answer: the notebook moves.
    let accepted = "";
    const shareId = await saveToAccount(page, {
      onConfirm: (message) => {
        accepted = message;
      },
    });
    expect(accepted).toBe(dismissed);
    expect(await localRecord(page, localUuid)).toBeNull();
    await expect(page).toHaveURL(new RegExp(`/mutable/${shareId}$`));
  });
});
