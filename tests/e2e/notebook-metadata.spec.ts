import { test, expect, Page } from "@playwright/test";
import { menuClick } from "./helpers/menu";
import { openMetadataPanel } from "./helpers/metadata";
import { createNotebook } from "./helpers/session";

/**
 * PRD-0051: the notebook metadata panel, and the fact that what it writes
 * actually reaches a link unfurl.
 *
 * The second half matters more than the first. These fields existed since
 * PRD-0050 and were read by the unfurl path; what was missing was any way to
 * set them, so a test that only checks IndexedDB would pass on a panel wired
 * to nothing.
 */

const BASE = "http://localhost:3111";

/** Value of a `<meta>` whose `property` or `name` is `key`, from raw HTML. */
function metaContent(html: string, key: string): string | null {
  const tag = html.match(
    new RegExp(`<meta[^>]+(?:property|name)="${key}"[^>]*>`, "i")
  )?.[0];
  return tag?.match(/content="([^"]*)"/i)?.[1] ?? null;
}

/** Clicks Save and waits for the durable-write toast. */
async function saveMetadata(page: Page, section: ReturnType<Page["locator"]>) {
  await section.locator("button", { hasText: "Save" }).click();
  await expect(
    page.locator("text=Notebook metadata saved").first()
  ).toBeVisible({ timeout: 15_000 });
}

test.describe("Notebook metadata panel (PRD-0051)", () => {
  test("description and tags persist and survive a reload", async ({
    page,
  }) => {
    test.setTimeout(120_000);
    await createNotebook(page);
    const url = page.url();
    const id = url.match(/\/local\/([a-f0-9-]+)/)![1];

    const section = await openMetadataPanel(page);
    await section
      .locator(".ironpad-metadata-textarea")
      .fill("Numerically integrating a cannonball.");
    await section
      .locator(".ironpad-search-input")
      .fill("simulation, autodiff");
    await saveMetadata(page, section);

    // Straight to the store: the toast means durably written, and this is
    // what every reader (home page, share, unfurl) actually consumes.
    await expect
      .poll(
        () =>
          page.evaluate(async (nid) => {
            const nb = await (window as any).IronpadStorage.getNotebook(nid);
            return { description: nb?.description, tags: nb?.tags };
          }, id),
        { timeout: 10_000 }
      )
      .toEqual({
        description: "Numerically integrating a cannonball.",
        tags: ["simulation", "autodiff"],
      });

    await page.goto(url);
    const reopened = await openMetadataPanel(page);
    await expect(reopened.locator(".ironpad-metadata-textarea")).toHaveValue(
      "Numerically integrating a cannonball."
    );
    await expect(reopened.locator(".ironpad-search-input")).toHaveValue(
      "simulation, autodiff"
    );
  });

  test("clearing a field removes it rather than leaving the old value", async ({
    page,
  }) => {
    // The doubled-option clear path: an empty box has to be able to unset a
    // value, which is a different code path from never having set one.
    test.setTimeout(120_000);
    await createNotebook(page);
    const id = page.url().match(/\/local\/([a-f0-9-]+)/)![1];

    let section = await openMetadataPanel(page);
    await section.locator(".ironpad-metadata-textarea").fill("Temporary.");
    await saveMetadata(page, section);

    await section.locator(".ironpad-metadata-textarea").fill("");
    await saveMetadata(page, section);

    await expect
      .poll(
        () =>
          page.evaluate(async (nid) => {
            const nb = await (window as any).IronpadStorage.getNotebook(nid);
            return nb?.description ?? null;
          }, id),
        { timeout: 10_000 }
      )
      .toBeNull();
  });

  test("a non-root-relative preview image is flagged before it can be saved blind", async ({
    page,
  }) => {
    // og_image_path() drops anything that isn't root-relative, silently, at
    // unfurl time. That is the worst moment to find out, so the panel says so
    // while you are typing.
    test.setTimeout(120_000);
    await createNotebook(page);
    const section = await openMetadataPanel(page);

    const imageField = section.locator(".ironpad-metadata-input");
    await imageField.fill("https://evil.example/x.png");
    await expect(section.locator(".ironpad-metadata-problem")).toBeVisible();

    await imageField.fill("/og-custom/mine.png");
    await expect(section.locator(".ironpad-metadata-problem")).toHaveCount(0);

    // The dimension inputs only appear once there is an override to describe;
    // the generated card is always 1200x630.
    await expect(section.locator(".ironpad-metadata-number")).toHaveCount(2);
  });

  test("the dimension fields stay hidden without an override image", async ({
    page,
  }) => {
    test.setTimeout(120_000);
    await createNotebook(page);
    const section = await openMetadataPanel(page);
    await expect(section.locator(".ironpad-metadata-number")).toHaveCount(0);
  });

  test("a description set in the editor reaches the shared copy's unfurl", async ({
    page,
    request,
  }) => {
    // The end-to-end claim of this feature: metadata authored in ironpad shows
    // up when someone pastes the link, instead of the generic fallback line.
    test.setTimeout(150_000);
    await createNotebook(page);

    const section = await openMetadataPanel(page);
    const description = `Set from the editor at ${Date.now()}.`;
    await section.locator(".ironpad-metadata-textarea").fill(description);
    await section.locator(".ironpad-search-input").fill("e2e");
    await saveMetadata(page, section);

    await menuClick(page, "Share Immutable");

    const toastBody = page.locator(".ironpad-toast-body", {
      hasText: "/shared/",
    });
    await expect(toastBody).toBeVisible({ timeout: 60_000 });
    const hash = (await toastBody.textContent())!.match(
      /\/shared\/([0-9a-f]{16})/
    )![1];

    const res = await request.get(`${BASE}/shared/${hash}`);
    expect(res.status()).toBe(200);
    const html = await res.text();

    expect(metaContent(html, "og:description")).toBe(description);
    expect(metaContent(html, "description")).toBe(description);
    // Untouched by the description change, and still the generated card.
    expect(metaContent(html, "og:image")).toBe(
      `${BASE}/og/shared/${hash}.png`
    );
    expect(metaContent(html, "og:image:width")).toBe("1200");
  });
});
