import { test, expect, type Page } from "@playwright/test";
import { trackJsErrors } from "./helpers/errors";

/**
 * `Plot` output must be readable in both themes.
 *
 * Before PRD-0065 the palette was hardcoded dark: `plot.rs` set
 * `COLOR_TEXT = #EAEAEA` and used it for every axis label, tick label, in-SVG
 * title and axis stroke. Measured on production in light theme that is
 * **1.11:1** against the `#f5f6fa` surface, where WCAG AA wants 4.5:1, so
 * every chart label was invisible. It reached six public notebooks and every
 * user notebook that plots, across `/public`, `/shared`, `/mutable` and three
 * `/embed` routes.
 *
 * The cell cannot fix this itself. It runs in a Web Worker and its SVG is
 * persisted into `saved_output` (PRD-0056), so a snapshot captured in dark
 * mode would stay dark forever no matter what the reader's theme is. The SVG
 * therefore carries `var(--ip-plot-*)` and the page themes it.
 *
 * Three properties hold that up, and this file pins all of them:
 *
 * 1. Contrast clears AA in BOTH themes, measured from the rendered chart
 *    rather than asserted against a hex constant. A test comparing strings
 *    would pass against an SVG nobody can read.
 *
 * 2. Custom properties only resolve for INLINE SVG. Panels are injected via
 *    `inner_html`, not as an `<img>` or a `data:` URI. If that ever changes,
 *    every `var()` silently resolves to nothing and the attribute drops.
 *
 * 3. A snapshot re-themes with no re-execution. Frozen bytes carrying `var()`
 *    are painted by whichever stylesheet reads them. Snapshots captured
 *    BEFORE this change hold baked hex and are fixed by the one-time
 *    recapture, not by CSS.
 */

/** WCAG 2.1 SC 1.4.3, normal-size text. */
const AA_NORMAL = 4.5;

/** Chart text is small; AA large-text (3:1) would be the wrong bar for it. */
const CONTRAST_FLOOR = AA_NORMAL;

/**
 * Contrast of the plot's text against the surface actually painted behind it,
 * computed in the page from resolved styles. Returns the worst ratio found
 * across the chart's text nodes, so one unreadable tick fails the test.
 */
async function worstPlotContrast(page: Page) {
  return page.evaluate(() => {
    const relLum = (r: number, g: number, b: number) => {
      const f = (v: number) => {
        v /= 255;
        return v <= 0.03928 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4);
      };
      return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b);
    };
    const rgb = (s: string): [number, number, number] => {
      const n = (s.match(/[\d.]+/g) ?? []).slice(0, 3).map(Number);
      return [n[0] ?? 0, n[1] ?? 0, n[2] ?? 0];
    };
    // The painted background behind an element: walk up past transparent.
    const painted = (el: Element): [number, number, number] => {
      let cur: Element | null = el;
      while (cur) {
        const c = getComputedStyle(cur).backgroundColor;
        if (c && !/rgba\(0, 0, 0, 0\)|transparent/.test(c)) return rgb(c);
        cur = cur.parentElement;
      }
      return [255, 255, 255];
    };

    const svg = [...document.querySelectorAll("svg")].find(
      (s) => !s.classList.contains("ironpad-icon") && s.querySelectorAll("text").length > 2,
    );
    if (!svg) return { found: false, worst: 0, sample: "", fill: "" };

    let worst = Infinity;
    let sample = "";
    let worstFill = "";
    for (const t of svg.querySelectorAll("text")) {
      const label = (t.textContent ?? "").trim();
      if (!label) continue;
      // getComputedStyle resolves var() for us, which is the whole point.
      const fill = getComputedStyle(t).fill;
      const a = relLum(...rgb(fill));
      const b = relLum(...painted(t));
      const ratio = (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
      if (ratio < worst) {
        worst = ratio;
        sample = label;
        worstFill = fill;
      }
    }
    return { found: true, worst, sample, fill: worstFill };
  });
}

/**
 * Flip the theme by clicking the real control, so `apply_theme` runs.
 *
 * Two traps here, both already paid for in this repo. Dark is the DEFAULT and
 * carries NO attribute: light sets `data-theme="light"` and dark REMOVES it,
 * so asserting `data-theme="dark"` would never pass. And
 * `.ironpad-theme-toggle` is SHARED with the view-only Cached/Fresh pills,
 * which once produced a spec that passed while measuring the wrong element,
 * so the selector is scoped to the header's `--compact` variant.
 */
async function setTheme(page: Page, theme: "light" | "dark") {
  const toggle = page.locator(".ironpad-theme-toggle--compact");
  await toggle.locator(`[title="${theme === "light" ? "Light" : "Dark"} mode"]`).click();
  const html = page.locator("html");
  if (theme === "light") {
    await expect(html).toHaveAttribute("data-theme", "light");
  } else {
    await expect(html).not.toHaveAttribute("data-theme", /.*/);
  }
}

test.describe("Plot theme", () => {
  for (const theme of ["light", "dark"] as const) {
    test(`chart text clears WCAG AA in ${theme} theme`, async ({ page }) => {
      const errors = trackJsErrors(page);
      await page.goto("/public/charts-with-plot");

      // Saved outputs (PRD-0056) render without running anything, which is
      // exactly the case that must re-theme: these bytes were captured once.
      const svg = page
        .locator(".view-only-output-svg svg")
        .filter({ hasNot: page.locator(".ironpad-icon") })
        .first();
      await expect(svg).toBeVisible({ timeout: 30_000 });

      await setTheme(page, theme);

      const result = await worstPlotContrast(page);
      expect(result.found, "found a plot SVG with text").toBe(true);
      expect(
        result.worst,
        `worst label "${result.sample}" resolved to ${result.fill} ` +
          `at ${result.worst.toFixed(2)}:1 in ${theme} theme`,
      ).toBeGreaterThanOrEqual(CONTRAST_FLOOR);

      expect(errors).toHaveLength(0);
    });
  }

  test("plot colors resolve through CSS, not baked hex", async ({ page }) => {
    await page.goto("/public/charts-with-plot");
    const svg = page.locator(".view-only-output-svg svg").first();
    await expect(svg).toBeVisible({ timeout: 30_000 });

    const markup = await page.evaluate(() => {
      const s = [...document.querySelectorAll("svg")].find(
        (x) => !x.classList.contains("ironpad-icon") && x.querySelectorAll("text").length > 2,
      );
      return s?.outerHTML ?? "";
    });

    expect(markup, "text/series colors come from custom properties").toContain("var(--ip-plot-");
    expect(markup, "the old hardcoded dark text color is gone").not.toContain("#EAEAEA");

    // Every var() must carry a fallback: CopyButton hands this SVG to the
    // clipboard and Download embeds it, and outside ironpad's stylesheet an
    // unresolved var() with no fallback drops the whole attribute.
    const varsWithoutFallback = [...markup.matchAll(/var\(--ip-plot-[a-z0-9-]+\s*\)/g)];
    expect(
      varsWithoutFallback.map((m) => m[0]),
      "a copied SVG still renders standalone",
    ).toEqual([]);
  });

  test("theme toggle re-themes a chart with no re-run", async ({ page }) => {
    await page.goto("/public/charts-with-plot");
    const svg = page.locator(".view-only-output-svg svg").first();
    await expect(svg).toBeVisible({ timeout: 30_000 });

    await setTheme(page, "light");
    const light = await worstPlotContrast(page);
    await setTheme(page, "dark");
    const dark = await worstPlotContrast(page);

    // Same bytes, different resolved paint. If these matched, the SVG would be
    // carrying a baked color and only one theme could be readable.
    expect(light.fill).not.toBe(dark.fill);
    expect(light.worst).toBeGreaterThanOrEqual(CONTRAST_FLOOR);
    expect(dark.worst).toBeGreaterThanOrEqual(CONTRAST_FLOOR);
  });

  test("panels are inline SVG, not an image", async ({ page }) => {
    // Custom properties do not resolve inside <img src="data:image/svg+xml">
    // or a background-image. This is the property the whole approach rests on.
    await page.goto("/public/charts-with-plot");
    await expect(page.locator(".view-only-output-svg svg").first()).toBeVisible({
      timeout: 30_000,
    });

    const inline = await page.evaluate(() => {
      const wrap = document.querySelector(".view-only-output-svg");
      return {
        hasInlineSvg: !!wrap?.querySelector("svg"),
        hasImg: !!wrap?.querySelector("img"),
      };
    });
    expect(inline.hasInlineSvg).toBe(true);
    expect(inline.hasImg, "an <img> would break var() resolution").toBe(false);
  });
});
