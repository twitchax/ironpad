import { test, expect } from "@playwright/test";
import { trackJsErrors } from "./helpers/errors";

/**
 * Smoke tests that open complex public notebooks, wait for the PRD-0040
 * first-party auto-run, and verify that output actually renders. These exercise the full pipeline:
 * SSR → hydration → server-side WASM compilation → client-side execution → output display.
 *
 * Each notebook chosen exercises a different output path:
 *   - Mandelbrot: BlobImage (pixel-rendered fractal images)
 *   - Game of Life: Simulation canvas (interactive animation)
 *   - Fourier Series: Interactive widgets + Simulation canvas
 */

// First compilation in a session downloads crates — allow plenty of time.
const NOTEBOOK_TIMEOUT = 600_000; // 10 min per test
const CELL_OUTPUT_TIMEOUT = 480_000; // 8 min for all cells to compile+run

test.describe("Notebook smoke tests", () => {
  test("mandelbrot notebook compiles and renders fractal images", async ({
    page,
  }) => {
    test.setTimeout(NOTEBOOK_TIMEOUT);
    const jsErrors = trackJsErrors(page);

    // Navigate to the Mandelbrot public notebook.
    await page.goto("/notebook/public/mandelbrot.ironpad");

    // Wait for the view-only container.
    await expect(page.locator(".view-only-notebook")).toBeVisible({
      timeout: 30_000,
    });

    // Public showcase notebooks auto-run on load (PRD-0040): first-party
    // trusted content, so no Run All click is needed.

    // The notebook has 2 code cells (cell_1, cell_2) that each produce a
    // BlobImage canvas output rendered as an <img> inside .view-only-output-display.
    // Wait for at least 2 timing badges (one per compiled code cell).
    const timingBadges = page.locator(".view-only-timing-badge");
    await expect(timingBadges.nth(1)).toBeVisible({
      timeout: CELL_OUTPUT_TIMEOUT,
    });

    // Verify output containers appeared for both code cells.
    // Wait for the second output to also render (execution may lag behind compilation).
    const outputs = page.locator(".view-only-output-display");
    await expect(outputs.first()).toBeVisible({ timeout: CELL_OUTPUT_TIMEOUT });

    // Verify rendered images (BlobImage output produces <img> tags).
    // Both cells produce fractal images — wait for at least 2.
    const images = page.locator(".view-only-output-display img");
    await expect(images.nth(1)).toBeVisible({ timeout: CELL_OUTPUT_TIMEOUT });
    const imageCount = await images.count();
    expect(imageCount).toBeGreaterThanOrEqual(2);

    // No errors should be visible.
    const errorPanels = page.locator(".view-only-error");
    expect(await errorPanels.count()).toBe(0);

    // Rayon validation: the Mandelbrot notebook uses par_iter, so
    // crossOriginIsolated must be true (SharedArrayBuffer required for threads).
    const isolated = await page.evaluate(() => window.crossOriginIsolated);
    expect(isolated).toBe(true);

    expect(jsErrors).toEqual([]);
  });

  // In CI, run only one smoke test to stay within time limits. The mandelbrot
  // test above exercises the full pipeline; these add coverage for different
  // output types but triple the CI time due to crate downloads.
  const ciTest = process.env.CI ? test.skip : test;

  ciTest("game of life notebook compiles and renders cell images", async ({
    page,
  }) => {
    test.setTimeout(NOTEBOOK_TIMEOUT);
    const jsErrors = trackJsErrors(page);

    // Navigate to the Game of Life public notebook.
    await page.goto("/notebook/public/game-of-life.ironpad");

    await expect(page.locator(".view-only-notebook")).toBeVisible({
      timeout: 30_000,
    });

    // Public showcase notebooks auto-run on load (PRD-0040): first-party
    // trusted content, so no Run All click is needed.

    // The notebook has 2 code cells that each produce BlobImage output:
    //   cell_1: "Simulation & Final State" — computes grid + renders final state image
    //   cell_2: "Evolution Filmstrip" — renders 5-frame filmstrip as a single image
    // Wait for timing badges indicating both cells compiled and ran.
    const timingBadges = page.locator(".view-only-timing-badge");
    await expect(timingBadges.nth(1)).toBeVisible({
      timeout: CELL_OUTPUT_TIMEOUT,
    });

    // Verify output containers appeared.
    const outputs = page.locator(".view-only-output-display");
    await expect(outputs.first()).toBeVisible({ timeout: CELL_OUTPUT_TIMEOUT });

    // Both cells produce BlobImage output rendered as <img> elements. Wait for
    // the SECOND image, not just the first: Run All executes cells
    // sequentially, so counting right after the first image races cell 2's
    // execution (timing badges only signal compilation).
    const images = page.locator(".view-only-output-display img");
    await expect(images.nth(1)).toBeVisible({ timeout: CELL_OUTPUT_TIMEOUT });
    const imageCount = await images.count();
    expect(imageCount).toBeGreaterThanOrEqual(2);

    // No errors should be visible.
    const errorPanels = page.locator(".view-only-error");
    expect(await errorPanels.count()).toBe(0);

    expect(jsErrors).toEqual([]);
  });

  ciTest("fourier series notebook compiles and renders interactive visualization", async ({
    page,
  }) => {
    test.setTimeout(NOTEBOOK_TIMEOUT);
    const jsErrors = trackJsErrors(page);

    // Navigate to the Fourier Series public notebook.
    await page.goto("/notebook/public/fourier-series.ironpad");

    await expect(page.locator(".view-only-notebook")).toBeVisible({
      timeout: 30_000,
    });

    // Public showcase notebooks auto-run on load (PRD-0040): first-party
    // trusted content, so no Run All click is needed.

    // The notebook has 2 code cells:
    //   cell_controls: interactive widget (slider/dropdown controls)
    //   cell_vis: Simulation (Fourier visualization → <canvas>)
    // Wait for timing badges indicating both code cells compiled.
    const timingBadges = page.locator(".view-only-timing-badge");
    await expect(timingBadges.nth(1)).toBeVisible({
      timeout: CELL_OUTPUT_TIMEOUT,
    });

    // Verify output containers appeared.
    const outputs = page.locator(".view-only-output-display");
    await expect(outputs.first()).toBeVisible({ timeout: CELL_OUTPUT_TIMEOUT });

    // The visualization cell should produce a <canvas> element (requires WASM
    // hydration, which may lag behind the timing badges).
    const canvases = page.locator(".view-only-output-display canvas");
    await expect(canvases.first()).toBeVisible({
      timeout: CELL_OUTPUT_TIMEOUT,
    });

    // The live simulation's tick path: every tick's frame is drawn by the
    // shared JS draw shim straight from the tick's `rgbBytes`. The fps
    // readout is what a live SimulationCanvas has and a saved snapshot's
    // one-frame replay does not.
    const sim = page
      .locator(".animation-canvas-container")
      .filter({ has: page.locator(".animation-fps-display") })
      .first();
    const frame = async () =>
      Number(
        (await sim.locator(".animation-frame-counter").textContent())?.match(/\d+/)?.[0] ?? 0,
      );
    await expect.poll(frame, { timeout: 60_000 }).toBeGreaterThanOrEqual(2);

    // Pause, let any in-flight tick land, then Step once: the counter moves
    // by exactly one and the frame the tick drew replaces the previous one.
    const controls = sim.locator(".animation-control-btn");
    await controls.nth(0).click();
    await page.waitForTimeout(1_000);
    const paused = await frame();
    const pixels = () =>
      sim.locator("canvas").evaluate((c: HTMLCanvasElement) => c.toDataURL());
    const before = await pixels();
    await controls.nth(1).click();
    await expect.poll(frame, { timeout: 10_000 }).toBe(paused + 1);
    expect(await pixels(), "the stepped frame was drawn").not.toBe(before);
    const alpha = await sim
      .locator("canvas")
      .evaluate(
        (c: HTMLCanvasElement) =>
          c.getContext("2d")!.getImageData(c.width / 2, c.height / 2, 1, 1).data[3],
      );
    expect(alpha, "a drawn pixel is opaque").toBe(255);

    // No errors should be visible.
    const errorPanels = page.locator(".view-only-error");
    expect(await errorPanels.count()).toBe(0);

    expect(jsErrors).toEqual([]);
  });
});
