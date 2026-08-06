import { test, expect } from "@playwright/test";

/**
 * PRD-0051: the oEmbed provider at /oembed, and the discovery link that points
 * consumers at it.
 *
 * Like social-preview.spec.ts, everything here goes through `request` rather
 * than `page`: an oEmbed consumer is a server fetching HTML and JSON, and it
 * never runs our JavaScript.
 */

const BASE = "http://localhost:3111";

/** The oEmbed endpoint URL for a canonical notebook path. */
function endpoint(path: string, extra = ""): string {
  return `${BASE}/oembed?url=${encodeURIComponent(`${BASE}${path}`)}${extra}`;
}

test.describe("oEmbed provider (PRD-0051)", () => {
  test("a public notebook resolves to an embeddable rich response", async ({
    request,
  }) => {
    const res = await request.get(endpoint("/public/welcome"));
    expect(res.status()).toBe(200);
    expect(res.headers()["content-type"]).toContain("application/json");

    const body = await res.json();
    expect(body.version).toBe("1.0");
    expect(body.type).toBe("rich");
    expect(body.provider_name).toBe("ironpad");
    expect(body.title).toBe("Welcome to ironpad");

    // The point of oEmbed over Open Graph: the consumer gets the running
    // notebook, not a picture of it. That means the chrome-less route.
    expect(body.html).toContain(`${BASE}/embed/public/welcome`);
    expect(body.html).toMatch(/^<iframe /);
    expect(body.height).toBeGreaterThan(0);
  });

  test("the page advertises the endpoint so consumers can discover it", async ({
    request,
  }) => {
    const res = await request.get(`${BASE}/public/welcome`);
    expect(res.status()).toBe(200);
    const html = await res.text();

    // Must be in the SSR'd first response for the same reason the og: tags
    // are: a consumer fetching this page runs no JavaScript.
    const link = html.match(
      /<link[^>]+type="application\/json\+oembed"[^>]*>/i
    )?.[0];
    expect(link, "no oembed discovery link in the raw body").toBeTruthy();

    // The href must carry the page URL percent-encoded as a query value; an
    // unencoded "/" or ":" would be read as structure by the endpoint.
    const href = link!.match(/href="([^"]*)"/i)![1];
    expect(href).toContain("/oembed?url=");
    expect(href).toContain(encodeURIComponent(`${BASE}/public/welcome`));

    // And following it must actually work.
    const followed = await request.get(href.replace(/&amp;/g, "&"));
    expect(followed.status()).toBe(200);
    expect((await followed.json()).type).toBe("rich");
  });

  test("the provider refuses URLs on other origins", async ({ request }) => {
    // A provider that embedded any URL would be an open redirect wearing an
    // iframe: the consumer trusts the returned HTML because it trusts us.
    for (const hostile of [
      "https://evil.example/public/welcome",
      `${BASE}.evil.example/public/welcome`,
      "/public/welcome",
    ]) {
      const res = await request.get(
        `${BASE}/oembed?url=${encodeURIComponent(hostile)}`
      );
      expect(res.status(), `${hostile} should not resolve`).toBe(404);
    }
  });

  test("classes without an embed route do not resolve", async ({ request }) => {
    // A private notebook lives only in one browser's IndexedDB; the home
    // page is not a notebook. (Mutable is embeddable as of PRD-0057 and is
    // covered by its own test below.)
    for (const path of ["/local/some-uuid", "/"]) {
      const res = await request.get(endpoint(path));
      expect(res.status(), `${path} should not resolve`).toBe(404);
    }
    // An unknown mutable id maps to the class but 404s on resolution.
    const res = await request.get(endpoint("/mutable/aaaa1111bbbb2222"));
    expect(res.status()).toBe(404);
  });

  test("a missing notebook is a 404, not an empty embed", async ({
    request,
  }) => {
    const res = await request.get(endpoint("/public/no-such-notebook"));
    expect(res.status()).toBe(404);
  });

  test("xml is refused rather than silently answered with json", async ({
    request,
  }) => {
    // The spec calls for 501 on a format the provider does not implement.
    const res = await request.get(endpoint("/public/welcome", "&format=xml"));
    expect(res.status()).toBe(501);
  });

  test("maxheight is honoured within bounds", async ({ request }) => {
    const height = async (max: number) => {
      const res = await request.get(
        endpoint("/public/welcome", `&maxheight=${max}`)
      );
      expect(res.status()).toBe(200);
      const body = await res.json();
      return body.height as number;
    };

    expect(await height(900)).toBe(900);
    // Clamped, so a careless consumer cannot produce an absurd frame.
    expect(await height(99999)).toBeLessThanOrEqual(4000);
    expect(await height(1)).toBeGreaterThanOrEqual(150);
  });
});
