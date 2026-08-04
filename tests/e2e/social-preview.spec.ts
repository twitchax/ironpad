import { test, expect, APIRequestContext, Page } from "@playwright/test";
import { createNotebook } from "./helpers/session";

/**
 * PRD-0050: link unfurls — Open Graph / Twitter Card metadata, the generated
 * preview cards at /og/*, and the crawler files.
 *
 * Everything here uses `request` rather than `page` on purpose. Reddit, X,
 * Slack, and Discord fetch the HTML and never run JavaScript, so a test that
 * asserts against the hydrated DOM would pass on markup no unfurler can see.
 * The raw response body IS the contract.
 */

const BASE = "http://localhost:3111";

/** Value of a `<meta>` whose `property` or `name` is `key`, from raw HTML. */
function metaContent(html: string, key: string): string | null {
  const pattern = new RegExp(
    `<meta[^>]+(?:property|name)="${key.replace(
      /[.*+?^${}()|[\]\\]/g,
      "\\$&"
    )}"[^>]*>`,
    "i"
  );
  const tag = html.match(pattern)?.[0];
  if (!tag) return null;
  return tag.match(/content="([^"]*)"/i)?.[1] ?? null;
}

async function rawHtml(
  request: APIRequestContext,
  path: string
): Promise<string> {
  const res = await request.get(`${BASE}${path}`);
  expect(res.status()).toBe(200);
  return res.text();
}

/**
 * Creates a private notebook, gives it `title`, shares it immutably, and
 * returns the minted 16-hex hash.
 *
 * A share needs no compiled cells, which is what makes this cheap enough to
 * run per-test. `fill` rather than `pressSequentially` because these titles
 * are deliberately hostile and must land verbatim, not as keystrokes an
 * editor might interpret.
 */
async function shareNotebookTitled(page: Page, title: string): Promise<string> {
  await createNotebook(page);

  await page.locator(".ironpad-notebook-title--editable").click();
  const titleInput = page.locator(".ironpad-header-title-input");
  await expect(titleInput).toBeVisible();
  await titleInput.fill(title);
  await titleInput.press("Enter");
  await expect(page.locator(".ironpad-notebook-title--editable")).toHaveText(
    title,
    { timeout: 10_000 }
  );

  await page.locator('button[title="Notebook menu"]').click();
  await page
    .locator(".ironpad-toolbar-dropdown-item", { hasText: "Share Immutable" })
    .click();

  // Filtered, not just `.ironpad-toast-body`: renaming fires its own "changes
  // saved" toast, so two are on screen and a bare locator is not strict-safe.
  const toastBody = page.locator(".ironpad-toast-body", {
    hasText: "/shared/",
  });
  await expect(toastBody).toBeVisible({ timeout: 60_000 });
  return (await toastBody.textContent())!.match(/\/shared\/([0-9a-f]{16})/)![1];
}

test.describe("Social preview metadata (PRD-0050)", () => {
  test("a public notebook serves its unfurl tags in the first response", async ({
    request,
  }) => {
    const html = await rawHtml(request, "/public/welcome");

    // The regression this guards: with the default streaming SSR mode these
    // tags are injected by a script after the head is already on the wire,
    // so the page looks right in a browser and unfurls as a bare domain.
    expect(metaContent(html, "og:title")).toBe("Welcome to ironpad");
    expect(metaContent(html, "og:type")).toBe("article");
    expect(metaContent(html, "og:site_name")).toBe("ironpad");
    expect(metaContent(html, "og:description")).toContain("Learn the basics");
    expect(html).toContain("<title>Welcome to ironpad · ironpad</title>");

    // Absolute, because a crawler has no document base to resolve against.
    expect(metaContent(html, "og:url")).toBe(`${BASE}/public/welcome`);
    expect(metaContent(html, "og:image")).toBe(
      `${BASE}/og/public/welcome.png`
    );
    expect(metaContent(html, "og:image:width")).toBe("1200");
    expect(metaContent(html, "og:image:height")).toBe("630");

    // Without this X shows a small square thumbnail, not the wide card.
    expect(metaContent(html, "twitter:card")).toBe("summary_large_image");
    expect(metaContent(html, "twitter:image")).toBe(
      `${BASE}/og/public/welcome.png`
    );

    // Showcase content is meant to be found.
    expect(metaContent(html, "robots")).toBeNull();
    // Attribute order is the renderer's business, so match on the pair.
    expect(html).toMatch(
      new RegExp(`<link[^>]*href="${BASE}/public/welcome"[^>]*rel="canonical"`)
    );
  });

  test("a legacy .ironpad link still advertises the canonical URL", async ({
    request,
  }) => {
    // The route accepts both name forms; the metadata must only ever name the
    // extension-less canonical one, or crawlers index the redirecting form.
    const html = await rawHtml(request, "/public/welcome.ironpad");
    expect(metaContent(html, "og:url")).toBe(`${BASE}/public/welcome`);
    expect(metaContent(html, "og:image")).toBe(`${BASE}/og/public/welcome.png`);
  });

  test("the home page unfurls as the site, not as a notebook", async ({
    request,
  }) => {
    const html = await rawHtml(request, "/");
    expect(metaContent(html, "og:type")).toBe("website");
    expect(metaContent(html, "og:title")).toBe("ironpad");
    expect(metaContent(html, "og:image")).toBe(`${BASE}/og/ironpad.png`);
    expect(metaContent(html, "robots")).toBeNull();
  });

  test("the generated card is a real 1200x630 PNG", async ({ request }) => {
    const res = await request.get(`${BASE}/og/public/welcome.png`);
    expect(res.status()).toBe(200);
    expect(res.headers()["content-type"]).toBe("image/png");
    // Unfurlers and CDNs may reuse the card; the site-wide default is
    // no-cache, so this route has to opt out of it explicitly.
    expect(res.headers()["cache-control"]).toContain("max-age");

    const body = await res.body();
    expect(body.subarray(0, 8)).toEqual(
      Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a])
    );
    // IHDR width/height, which is what an unfurler reads to size the card.
    expect(body.readUInt32BE(16)).toBe(1200);
    expect(body.readUInt32BE(20)).toBe(630);
  });

  test("the site card renders too", async ({ request }) => {
    const res = await request.get(`${BASE}/og/ironpad.png`);
    expect(res.status()).toBe(200);
    expect(res.headers()["content-type"]).toBe("image/png");
    expect((await res.body()).readUInt32BE(16)).toBe(1200);
  });

  test("card routes reject unknown classes and path traversal", async ({
    request,
  }) => {
    for (const path of [
      "/og/local/whatever.png",
      "/og/public/nope-does-not-exist.png",
      "/og/public/welcome",
      "/og/public/%2e%2e%2f%2e%2e%2fetc%2fpasswd.png",
    ]) {
      const res = await request.get(`${BASE}${path}`, {
        maxRedirects: 0,
      });
      expect(res.status(), `${path} should not resolve`).toBe(404);
    }
  });

  test("every response carries a CSP that does not break embedding", async ({
    request,
  }) => {
    // PRD-0051. Deliberately narrow: no script-src, because without nonce
    // plumbing through leptos_meta and Monaco it could only be
    // 'unsafe-inline', which permits the very injection a CSP is for.
    for (const path of ["/", "/public/welcome", "/embed/public/welcome"]) {
      const res = await request.get(`${BASE}${path}`);
      const csp = res.headers()["content-security-policy"];
      expect(csp, `no CSP on ${path}`).toBeTruthy();
      expect(csp).toContain("object-src 'none'");
      expect(csp).toContain("base-uri 'self'");
      expect(csp).toContain("form-action 'self'");
      // /embed/* exists to be framed by third parties (PRD-0039); any
      // frame-ancestors value would break that outright.
      expect(csp).not.toContain("frame-ancestors");
      expect(csp).not.toContain("unsafe-inline");
    }
  });

  test("robots.txt points crawlers at the sitemap", async ({ request }) => {
    const res = await request.get(`${BASE}/robots.txt`);
    expect(res.status()).toBe(200);
    const body = await res.text();

    expect(body).toContain("User-agent: *");
    expect(body).toContain(`Sitemap: ${BASE}/sitemap.xml`);
    expect(body).toContain("Disallow: /embed/");
    // Blocking share routes here would stop unfurlers fetching them at all;
    // those pages use `noindex` instead.
    expect(body).not.toContain("Disallow: /shared");
  });

  test("sitemap lists the showcase notebooks by canonical route", async ({
    request,
  }) => {
    const res = await request.get(`${BASE}/sitemap.xml`);
    expect(res.status()).toBe(200);
    expect(res.headers()["content-type"]).toContain("xml");

    const body = await res.text();
    expect(body).toContain(`<loc>${BASE}/</loc>`);
    expect(body).toContain(`<loc>${BASE}/public/welcome</loc>`);
    expect(body).toContain(`<loc>${BASE}/public/cannon</loc>`);
    // Canonical form only: the extension-carrying URL redirects.
    expect(body).not.toContain(".ironpad");
  });

  test("a hostile notebook title cannot inject markup into the head", async ({
    page,
    request,
  }) => {
    // Regression guard for a stored XSS shipped in v0.13.0. leptos_meta
    // splices the title into the SSR head as raw text and `<title>` is
    // RCDATA, so an injected `</title>` closed the element and the script
    // after it ran on ironpad's own origin — with first-party access to
    // IndexedDB and the session cookie's ambient authority. Share uploads
    // are unauthenticated, so any visitor could be handed such a link.
    test.setTimeout(120_000);

    const hash = await shareNotebookTitled(
      page,
      "</title><script>window.PWNED=1</script>"
    );
    const html = await rawHtml(request, `/shared/${hash}`);

    // The text between <title> and the FIRST </title>. If the payload broke
    // out, that boundary lands early and the rest of the payload is markup.
    const open = html.indexOf("<title>") + "<title>".length;
    const inner = html.slice(open, html.indexOf("</title>", open));

    // The property is that no MARKUP survived, not that the payload's text is
    // gone: "window.PWNED=1" stays as inert characters in the title, which is
    // exactly right. What must not exist is a tag.
    expect(inner).not.toContain("<");
    expect(inner).not.toContain(">");
    expect(inner).toContain("ironpad");
    // And nothing anywhere in the document became a live script.
    expect(html).not.toContain("<script>window.PWNED");

    // The card for the same notebook must still render rather than 500.
    const card = await request.get(`${BASE}/og/shared/${hash}.png`);
    expect(card.status()).toBe(200);
  });

  test("a shared notebook unfurls but stays out of search indexes", async ({
    page,
    request,
  }) => {
    test.setTimeout(120_000);

    const title = `Preview subject ${Date.now()}`;
    const hash = await shareNotebookTitled(page, title);

    const html = await rawHtml(request, `/shared/${hash}`);
    expect(metaContent(html, "og:title")).toBe(title);
    expect(metaContent(html, "og:url")).toBe(`${BASE}/shared/${hash}`);
    expect(metaContent(html, "og:image")).toBe(
      `${BASE}/og/shared/${hash}.png`
    );
    // Unlisted by construction: a preview, but not a search result.
    expect(metaContent(html, "robots")).toContain("noindex");

    const card = await request.get(`${BASE}/og/shared/${hash}.png`);
    expect(card.status()).toBe(200);
    expect((await card.body()).readUInt32BE(16)).toBe(1200);
  });
});
