import { test, expect } from "@playwright/test";
import { trackJsErrors } from "./helpers/errors";

test.describe("Public notebooks", () => {
  test("home page shows public notebook badges", async ({ page }) => {
    // Collect JS errors during navigation.
    const jsErrors = trackJsErrors(page);

    // Navigate to home page.
    await page.goto("/");
    await expect(page.locator(".ironpad-home")).toBeVisible();

    // Verify at least one public notebook badge is visible.
    const publicBadge = page.locator(".ironpad-notebook-badge.public");
    await expect(publicBadge.first()).toBeVisible({ timeout: 10_000 });

    // Verify no JS errors occurred.
    expect(jsErrors).toEqual([]);
  });

  test("public notebook loads with cells and fork button", async ({
    page,
  }) => {
    // Collect JS errors (filter known WASM hydration noise).
    const jsErrors = trackJsErrors(page);

    // Navigate directly to the Welcome public notebook.
    await page.goto("/notebook/public/welcome.ironpad");

    // Verify the view-only notebook container is visible.
    await expect(page.locator(".view-only-notebook")).toBeVisible({
      timeout: 15_000,
    });

    // Verify cells are rendered.
    const cells = page.locator(".view-only-cell");
    await expect(cells.first()).toBeVisible({ timeout: 10_000 });
    const count = await cells.count();
    expect(count).toBeGreaterThanOrEqual(1);

    // Verify the "Fork to Private" button is present.
    const forkButton = page.locator(".fork-button");
    await expect(forkButton).toBeVisible();
    await expect(forkButton).toContainText("Fork to Private");

    // Verify no JS errors occurred.
    expect(jsErrors).toEqual([]);
  });

  test("fork button navigates to new private notebook", async ({ page }) => {
    // Forking may take a moment — generous timeout.
    test.setTimeout(60_000);

    // Collect JS errors (filter known WASM hydration noise).
    const jsErrors = trackJsErrors(page);

    // Navigate to the Welcome public notebook.
    await page.goto("/notebook/public/welcome.ironpad");
    await expect(page.locator(".view-only-notebook")).toBeVisible({
      timeout: 15_000,
    });

    // Wait for WASM hydration to complete (fork requires client-side code).
    // The view-only cells are server-rendered; wait for the fork button's
    // click handler to be wired up by checking that IronpadStorage is available.
    await page.waitForFunction(() => !!(window as any).IronpadStorage, null, {
      timeout: 15_000,
    });

    // IronpadStorage present means storage.js loaded, not that Leptos
    // hydration wired the button — give hydration the suite-convention beat.
    await page.waitForTimeout(3_000); // hydration (suite convention)

    // Click the fork button.
    const forkButton = page.locator(".fork-button");
    await expect(forkButton).toBeVisible();
    await forkButton.click();

    // Verify navigation to a new private notebook editor.
    // The fork URL must NOT match /notebook/public/ (it should be /notebook/{uuid}).
    await expect(page).toHaveURL(/\/local\/[a-f0-9-]+/, {
      timeout: 15_000,
    });
    await expect(page.locator(".ironpad-editor")).toBeVisible({
      timeout: 15_000,
    });

    // Verify the forked notebook has cells.
    const cells = page.locator(".ironpad-cell-card");
    await expect(cells.first()).toBeVisible({ timeout: 10_000 });

    // Verify no JS errors occurred.
    expect(jsErrors).toEqual([]);
  });
});

test.describe("View-only polish", () => {
  test("shared source and dependencies render as a collapsed appendix", async ({
    page,
  }) => {
    // The borrows notebook ships both shared source and shared Cargo.toml.
    await page.goto("/notebook/public/borrows.ironpad");
    await expect(page.locator(".view-only-notebook")).toBeVisible({
      timeout: 30_000,
    });

    const headers = page.locator(".view-only-shared-header");
    await expect(headers).toHaveCount(2);
    await expect(headers.nth(0)).toContainText("Shared Source");
    await expect(headers.nth(1)).toContainText("Shared Dependencies");

    // Collapsed by default; expanding mounts a read-only Monaco lazily.
    await expect(page.locator(".view-only-shared-body")).toHaveCount(0);
    await page.waitForTimeout(3_000); // hydration
    await headers.nth(0).click();
    await expect(
      page.locator(".view-only-shared-body .monaco-editor")
    ).toBeVisible({ timeout: 15_000 });
  });

  test("toolbar controls render below the title as single-line pills", async ({
    page,
  }) => {
    // Long titles used to strand buttons on ragged wrap rows (and squeeze
    // them into multi-line pills). Controls now live in their own row under
    // the title, each keeping its single-line height.
    await page.setViewportSize({ width: 700, height: 800 });
    await page.goto("/notebook/public/borrows.ironpad");
    await expect(page.locator(".view-only-notebook")).toBeVisible({
      timeout: 30_000,
    });

    const titleBottom = await page
      .locator(".view-only-title")
      .evaluate((el) => el.getBoundingClientRect().bottom);
    const controlsTop = await page
      .locator(".view-only-toolbar-controls")
      .evaluate((el) => el.getBoundingClientRect().top);
    expect(controlsTop).toBeGreaterThanOrEqual(titleBottom);

    const h = await page
      .locator(".run-all-button")
      .evaluate((el) => el.getBoundingClientRect().height);
    expect(h).toBeLessThan(45);
  });

  test("autorun continues past a deliberately failing cell", async ({
    page,
  }) => {
    // dynosaur's first code cell fails on purpose (the dyn-AFIT wall); the
    // two cells behind it must still compile and run, and the teaching
    // failure must be visible. The queue used to abort on any error,
    // stranding every later cell.
    test.setTimeout(600_000);
    await page.goto("/notebook/public/dynosaur.ironpad");
    await expect(page.locator(".view-only-notebook")).toBeVisible({
      timeout: 30_000,
    });

    // Both succeeding cells produce timing badges.
    await expect(page.locator(".view-only-timing-badge").nth(1)).toBeVisible({
      timeout: 480_000,
    });
    // The deliberate failure renders inline rather than being swallowed.
    expect(await page.locator(".view-only-error").count()).toBeGreaterThan(0);
  });
});

test.describe("Stylesheet token hygiene", () => {
  test("compiled CSS references only the app's --ip- palette", async ({
    request,
  }) => {
    // Thaw's --colorXxx tokens live on its wrapper element and fail SILENTLY
    // when a name doesn't exist (transparent backgrounds, currentColor
    // borders — the invisible-popover bug). App styles must use the --ip-*
    // palette, which is defined at :root with light-theme overrides.
    const css = await (await request.get("/pkg/ironpad.css")).text();
    const foreign = css.match(/var\(--color[A-Za-z0-9]+/g);
    expect(foreign).toBeNull();
  });
});

test.describe("Per-cell collapse state", () => {
  test("cells without collapse flags render code expanded", async ({
    page,
  }) => {
    // The blog-style notebooks are ABOUT the code: their cells carry no
    // collapsed flags, so sources are visible without any clicks (open is
    // the default).
    await page.goto("/notebook/public/dynosaur.ironpad");
    await expect(page.locator(".view-only-notebook")).toBeVisible({
      timeout: 30_000,
    });
    await expect(
      page.locator(".ironpad-cell-body--collapsed")
    ).toHaveCount(0);
    // A code cell's Monaco is visible immediately.
    await expect(
      page.locator(".view-only-cell .monaco-editor").first()
    ).toBeVisible({ timeout: 15_000 });
  });

  test("cells saved with collapsed: true load collapsed", async ({
    page,
  }) => {
    // Story-style notebooks mark their code cells collapsed per cell (the
    // migration from the old notebook-level expand_code setting).
    await page.goto("/notebook/public/welcome.ironpad");
    await expect(page.locator(".view-only-notebook")).toBeVisible({
      timeout: 30_000,
    });
    const collapsed = await page
      .locator(".ironpad-cell-body--collapsed")
      .count();
    expect(collapsed).toBeGreaterThan(0);
  });
});
