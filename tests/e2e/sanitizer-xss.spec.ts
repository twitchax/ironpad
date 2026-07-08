import { test, expect } from "@playwright/test";

// Extend Window to include IronpadStorage (defined in public/storage.js).
declare global {
  interface Window {
    IronpadStorage: {
      importNotebook: (json: string) => Promise<{ id: string; title: string }>;
    };
  }
}

// ── uat-003 (PRD-0038 T-005/T-006): sanitizer clickjacking neutralization ─────
//
// A shared/public notebook is viewed by *other* people, and cell/markdown output
// is injected via `inner_html`. Before T-005, `style=` passed through unfiltered,
// so an attacker cell could paint a full-viewport `position:fixed` overlay to
// hijack the viewer's clicks. This drives the sanitizer at the DOM level: the
// overlay's positioning/stacking CSS must be stripped while ordinary
// presentation (colour) survives — proving it's a CSS-property *filter*, not a
// blanket drop. Markdown output goes through the same `sanitize_html` as the
// shared/public viewer, so a markdown cell is a faithful, compile-free vector.

test("sanitizer strips a position:fixed overlay from cell output (clickjacking)", async ({
  page,
}) => {
  test.setTimeout(60_000);

  const jsErrors: string[] = [];
  page.on("pageerror", (error) => {
    if (!error.message.includes("unreachable")) {
      jsErrors.push(error.message);
    }
  });

  await page.goto("/");
  await expect(page.locator(".ironpad-home")).toBeVisible();

  const overlay =
    'Overlay <a class="hijack" ' +
    'style="position:fixed;inset:0;z-index:99999;background:rgb(255,0,0);color:rgb(0,0,255)" ' +
    'href="https://evil.example/">HIJACK</a>';

  const fixture = {
    version: 1,
    id: "00000000-0000-0000-0000-e2exsstest01",
    title: "XSS overlay test",
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    shared_cargo_toml: "",
    cells: [
      {
        id: "md0",
        order: 0,
        label: "md",
        cell_type: "Markdown",
        source: overlay,
        cargo_toml: "",
      },
    ],
  };

  const imported = await page.evaluate(
    (json) => window.IronpadStorage.importNotebook(json),
    JSON.stringify(fixture),
  );

  await page.goto(`/notebook/${imported.id}`);
  await expect(page.locator(".ironpad-editor")).toBeVisible({ timeout: 15_000 });

  // The anchor survives (it's allowed markup), but rendered inside the sanitized
  // markdown preview.
  const anchor = page.locator(".ironpad-markdown-cell-preview a.hijack");
  await expect(anchor).toBeVisible({ timeout: 15_000 });

  const styles = await anchor.evaluate((el) => {
    const s = getComputedStyle(el);
    return { position: s.position, zIndex: s.zIndex, backgroundColor: s.backgroundColor };
  });

  // Clickjacking primitives are stripped: no fixed positioning, no stacking lift.
  expect(styles.position).toBe("static");
  expect(styles.zIndex).toBe("auto");
  // …while legitimate presentation survives (a filter, not a blanket drop).
  expect(styles.backgroundColor).toBe("rgb(255, 0, 0)");

  expect(jsErrors).toEqual([]);
});
