import { test, expect, type Page } from "@playwright/test";
import { loginTestUser } from "./helpers/auth";

/**
 * Narrow-viewport and input-modality behaviour.
 *
 * These exist because a stylesheet audit cannot see any of it. The plot-sizing
 * regression below shipped for months looking correct in the source: the
 * editor's rule scaled `Plot` output to its container, and the view-only rule
 * beside it included the shared hover mixin but not the two sizing
 * declarations, so every reader-facing surface (/public, /shared, /mutable and
 * all three /embed routes) clipped a third of every chart. Both rules read
 * fine. Only a rendered width tells the truth, so these assert measured boxes.
 */

const PHONE = { width: 390, height: 844 };

/** Widest right-edge overshoot of any element past the viewport, in CSS px. */
async function horizontalOverflow(page: Page): Promise<number> {
  return page.evaluate(() => {
    const vw = document.documentElement.clientWidth;
    return Math.max(0, document.documentElement.scrollWidth - vw);
  });
}

test.describe("Narrow viewports", () => {
  test.use({ viewport: PHONE });

  test("plot output fits its container on a phone", async ({ page }) => {
    // charts-with-plot carries saved outputs (PRD-0056), so the SVG renders
    // from the snapshot and this test never waits on a compile.
    await page.goto("/public/charts-with-plot");
    await expect(page.locator(".view-only-notebook")).toBeVisible({
      timeout: 15_000,
    });

    const panel = page.locator(".view-only-output-svg").first();
    await expect(panel).toBeVisible({ timeout: 15_000 });

    const fit = await panel.evaluate((el) => {
      const svg = el.querySelector("svg:not(.ironpad-icon)");
      return {
        // The authored size, which is what makes this test meaningful: the SVG
        // ships width="800" and is being asked to live in ~340px.
        authoredWidth: svg?.getAttribute("width") ?? null,
        hasViewBox: !!svg?.getAttribute("viewBox"),
        renderedWidth: Math.round(svg?.getBoundingClientRect().width ?? 0),
        containerWidth: el.clientWidth,
        hiddenPx: el.scrollWidth - el.clientWidth,
      };
    });

    expect(fit.authoredWidth).toBe("800");
    // Scaling down is only lossless because the viewBox is there to scale into.
    expect(fit.hasViewBox).toBe(true);
    expect(fit.hiddenPx).toBe(0);
    expect(fit.renderedWidth).toBeLessThanOrEqual(fit.containerWidth);
    // Guard the other direction too: a chart collapsed to a sliver would also
    // satisfy "fits", and would be just as unreadable as one that overflows.
    expect(fit.renderedWidth).toBeGreaterThan(fit.containerWidth * 0.5);
  });

  test("slider rows keep their label and value on a phone", async ({
    page,
  }) => {
    await page.goto("/public/double-pendulum");
    await expect(page.locator(".view-only-notebook")).toBeVisible({
      timeout: 15_000,
    });

    const row = page.locator(".ironpad-interactive-widget").first();
    await expect(row).toBeVisible({ timeout: 15_000 });

    const fit = await row.evaluate((el) => {
      const box = el.getBoundingClientRect();
      const label = el.querySelector(".ironpad-widget-label");
      const value = el.querySelector(".ironpad-widget-value");
      const valueBox = value?.getBoundingClientRect();
      return {
        hiddenPx: el.scrollWidth - el.clientWidth,
        labelWidth: Math.round(label?.getBoundingClientRect().width ?? 0),
        // The readout is the point of moving the slider; it used to sit past
        // the row's right border where nothing could show it.
        valueInsideRow: valueBox ? valueBox.right <= box.right + 1 : false,
        valueWidth: Math.round(valueBox?.width ?? 0),
      };
    });

    expect(fit.hiddenPx).toBe(0);
    expect(fit.valueInsideRow).toBe(true);
    expect(fit.valueWidth).toBeGreaterThan(0);
    // The label may ellipsize, but it may not vanish: a slider with no name is
    // not a control, and `min-width: 0` alone would permit exactly that.
    expect(fit.labelWidth).toBeGreaterThan(40);
  });

  test("reader and editor routes do not scroll sideways", async ({ page }) => {
    for (const route of ["/", "/public/charts-with-plot", "/public/welcome"]) {
      await page.goto(route);
      await expect(page.locator(".ironpad-header")).toBeVisible({
        timeout: 15_000,
      });
      expect(
        await horizontalOverflow(page),
        `${route} overflows horizontally at ${PHONE.width}px`,
      ).toBe(0);
    }
  });
});

test.describe("Touch pointers", () => {
  test.use({ viewport: PHONE, hasTouch: true, isMobile: true });

  test("every visible control meets the 44px target size", async ({ page }) => {
    await page.goto("/public/charts-with-plot");
    await expect(page.locator(".view-only-notebook")).toBeVisible({
      timeout: 15_000,
    });

    // Assert the emulation actually took, so a Playwright default change turns
    // this into a failure rather than a test that silently proves nothing.
    const coarse = await page.evaluate(
      () => window.matchMedia("(pointer: coarse)").matches,
    );
    expect(coarse).toBe(true);

    // Deliberately every button and link on the page rather than a named list.
    // A named list only ever covers the controls someone remembered, and the
    // first draft of this test named `.ironpad-theme-toggle-segment` believing
    // it meant the header's icon squares — that class is shared with the
    // view-only Cached/Fresh pills, so it silently measured something else.
    const undersized = await page.evaluate(() => {
      const out: { cls: string; size: string; text: string }[] = [];
      for (const el of document.querySelectorAll<HTMLElement>(
        "button, a[href], .ironpad-drag-handle",
      )) {
        const r = el.getBoundingClientRect();
        if (r.width === 0 || r.height === 0) continue;
        // Links inside prose are text, not targets; the guideline exempts them
        // for exactly that reason.
        if (el.closest(".view-only-markdown, .ironpad-markdown-body")) continue;
        if (r.width < 44 || r.height < 44) {
          out.push({
            cls: el.className,
            size: `${Math.round(r.width)}x${Math.round(r.height)}`,
            text: (el.textContent ?? "").trim().slice(0, 20),
          });
        }
      }
      return out;
    });

    expect(undersized).toEqual([]);
  });

  test("the signed-in header control is one tappable stack", async ({
    page,
  }) => {
    // Signed-in is a distinct control from signed-out and renders only with a
    // session, so the sweep above cannot see it. It hid a worse problem than a
    // small box: the portrait was inert and only the 0.6rem "Sign out" line
    // was a link.
    await loginTestUser(page, "responsive");

    const auth = page.locator(".ironpad-auth");
    await expect(auth).toBeVisible({ timeout: 15_000 });

    const control = await auth.evaluate((el) => {
      const r = el.getBoundingClientRect();
      const portrait = el.querySelector(
        ".ironpad-auth-avatar, .ironpad-auth-signin-avatar",
      );
      return {
        tag: el.tagName,
        width: Math.round(r.width),
        height: Math.round(r.height),
        // The portrait must answer to the same click as the label.
        portraitInsideLink: !!portrait && el.contains(portrait),
        nestedLinks: el.querySelectorAll("a").length,
      };
    });

    expect(control.tag).toBe("A");
    expect(control.portraitInsideLink).toBe(true);
    // An <a> inside an <a> is invalid and does not nest in the parsed DOM, so
    // this also guards the markup, not only the target size.
    expect(control.nestedLinks).toBe(0);
    expect(control.width).toBeGreaterThanOrEqual(44);
    expect(control.height).toBeGreaterThanOrEqual(44);
  });
});

test.describe("Reduced motion", () => {
  test("chrome animations stop and cell output keeps moving", async ({
    page,
  }) => {
    // `emulateMedia` rather than `test.use({ reducedMotion })`: the fixture form
    // did not reach the page here (the guard below caught it returning false),
    // and a preference test that silently runs without the preference is worse
    // than no test at all.
    await page.emulateMedia({ reducedMotion: "reduce" });
    await page.goto("/");
    await expect(page.locator(".ironpad-home")).toBeVisible({
      timeout: 15_000,
    });

    // Same guard as the touch test: prove the emulation is on, so a failure
    // below means the stylesheet is wrong rather than the harness being idle.
    const reduced = await page.evaluate(
      () => window.matchMedia("(prefers-reduced-motion: reduce)").matches,
    );
    expect(reduced).toBe(true);

    const stopped = await page.evaluate(() => {
      // Probe the rule rather than hunting for a status badge in a transient
      // state: mint a throwaway element carrying each animated class and read
      // back what the cascade decided.
      const check = (cls: string) => {
        const el = document.createElement("div");
        el.className = cls;
        document.body.appendChild(el);
        const name = getComputedStyle(el).animationName;
        el.remove();
        return { cls, animationName: name };
      };
      return [
        "ironpad-skeleton-item",
        "ironpad-cell-status--running",
        "ironpad-stale-indicator--pending",
        "ironpad-toast",
      ].map(check);
    });

    for (const probe of stopped) {
      expect(probe.animationName, `${probe.cls} still animates`).toBe("none");
    }

    // The preference must not reach into cell output. A reader who asked for
    // less motion still came for the double pendulum, and the blanket
    // `*, *::before, *::after` reset would have stopped it.
    await page.goto("/public/double-pendulum");
    await expect(page.locator(".view-only-notebook")).toBeVisible({
      timeout: 15_000,
    });
    const outputAnimationsIntact = await page.evaluate(() => {
      const probe = document.createElement("div");
      probe.style.animation = "spin 2s linear infinite";
      document.querySelector(".view-only-output")?.appendChild(probe);
      const duration = getComputedStyle(probe).animationDuration;
      probe.remove();
      return duration;
    });
    expect(outputAnimationsIntact).toBe("2s");
  });
});

test.describe("Studio frame", () => {
  /**
   * The rail is chrome: it meets the header, the footer and the window edge.
   *
   * Its first shape nested it in a row WITH the cell list but BELOW the
   * toolbar, which produced both halves of one bug. It started 139px under
   * the header instead of touching it, and the cells ended up centred in a
   * different box than their own title, 122px apart at 1440px.
   *
   * Only rendered geometry can see any of this. Both stylesheets read
   * correctly the whole time.
   */
  test("the rail spans header to footer and meets the left edge", async ({
    page,
  }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.goto("/public/linux-cells");
    await expect(page.locator(".view-only-notebook")).toBeVisible({
      timeout: 30_000,
    });

    const geom = await page.evaluate(() => {
      const box = (s: string) => {
        const el = document.querySelector(s);
        if (!el) return null;
        const r = el.getBoundingClientRect();
        return { x: r.x, y: r.y, right: r.right, bottom: r.bottom };
      };
      return {
        header: box(".ironpad-header"),
        footer: box(".ironpad-status-bar"),
        rail: box(".ip-rail"),
        toolbar: box(".view-only-toolbar"),
        cell: box(".view-only-cell"),
        overflowX:
          document.documentElement.scrollWidth -
          document.documentElement.clientWidth,
      };
    });

    // Flush left, and vertically it exactly fills the gap between the two
    // bars. Asserting the SHARED edges rather than a height: a rail that
    // merely happened to be 824px tall while floating would pass a height
    // check and fail this one.
    expect(geom.rail!.x).toBe(0);
    expect(Math.round(geom.rail!.y)).toBe(Math.round(geom.header!.bottom));
    expect(Math.round(geom.rail!.bottom)).toBe(Math.round(geom.footer!.y));

    // The title and the cells under it share a centre. They are built to:
    // the cell list's max-width plus its own gutters is the toolbar's
    // max-width, so any drift means they are being centred in different
    // boxes again.
    const centre = (b: { x: number; right: number }) => (b.x + b.right) / 2;
    expect(Math.abs(centre(geom.toolbar!) - centre(geom.cell!))).toBeLessThan(
      2,
    );

    expect(geom.overflowX).toBe(0);
  });

  /**
   * The shell gutter shrinks at 1024px and again at 768px, and the frame
   * cancels it to go full-bleed. A hard-coded cancellation is correct at one
   * width and wrong at the other two: the first version hung the rail 8px off
   * the left edge at 1000px, which is why this sweeps widths instead of
   * checking the desktop case twice.
   */
  for (const width of [1440, 1200, 1024, 1000, 768, 390]) {
    test(`the frame cancels the shell gutter exactly at ${width}px`, async ({
      page,
    }) => {
      await page.setViewportSize({ width, height: 800 });
      await page.goto("/public/linux-cells");
      await expect(page.locator(".view-only-notebook")).toBeVisible({
        timeout: 30_000,
      });

      const geom = await page.evaluate(() => {
        const el = document.querySelector(".view-only-notebook")!;
        const r = el.getBoundingClientRect();
        return {
          x: r.x,
          right: r.right,
          inner: window.innerWidth,
          overflowX:
            document.documentElement.scrollWidth -
            document.documentElement.clientWidth,
        };
      });

      expect(geom.x, `frame overhangs the left edge at ${width}px`).toBe(0);
      expect(Math.round(geom.right)).toBe(geom.inner);
      expect(geom.overflowX).toBe(0);
    });
  }
});
