import { test, expect, Browser, Page } from "@playwright/test";
import { loginTestUser } from "./helpers/auth";
import { menuClick } from "./helpers/menu";
import { shareMutable } from "./helpers/mutable";

/**
 * Autorun follows the route's trust boundary (PRD-0040), and a notebook cannot
 * talk its way past it.
 *
 * Shared, mutable and embedded notebooks are arbitrary user content, so they
 * never execute on load: only the route may grant autorun, and only for
 * first-party content (`/public`, or a public embed whose embedder opted in).
 * The viewer used to OR the route's grant with the notebook's own
 * `reactive_mode` flag, which let any shared notebook opt itself into running
 * in every reader's browser just by being saved in Reactive Mode.
 *
 * "Ran" is read off the network, not the DOM: a run in a fresh browser
 * context has an empty local blob cache, so it must fetch a share snapshot
 * (`/share-blobs/`) or compile (`compile_cell`). Each surface gets its own
 * fresh context for exactly that reason, and each ends with a Run All click
 * that must produce that traffic, so "nothing happened" cannot just mean "not
 * hydrated yet".
 */

/** A one-cell notebook saved in Reactive Mode. */
const REACTIVE_FIXTURE = {
  version: 1,
  id: "00000000-0000-0000-0000-e2eautorun001",
  title: "Reactive autorun probe",
  created_at: "2026-01-01T00:00:00Z",
  updated_at: "2026-01-01T00:00:00Z",
  reactive_mode: true,
  shared_cargo_toml: "",
  cells: [
    {
      id: "c0",
      order: 0,
      label: "answer",
      cell_type: "Code",
      source: 'CellOutput::text(format!("{}", 41 + 1))',
      cargo_toml: "",
    },
  ],
};

declare global {
  interface Window {
    IronpadStorage: {
      importNotebook: (json: string) => Promise<{ id: string; title: string }>;
    };
  }
}

const isRunTraffic = (url: string) =>
  url.includes("compile_cell") || url.includes("/share-blobs/");

/**
 * Open `path` in a fresh anonymous context, assert nothing runs on load, then
 * prove the page is live by running it by hand.
 */
async function expectIdleOnLoad(browser: Browser, path: string): Promise<void> {
  const context = await browser.newContext();
  const page = await context.newPage();
  const runs: string[] = [];
  page.on("request", (req) => {
    if (isRunTraffic(req.url())) runs.push(req.url());
  });

  await page.goto(path);
  await expect(page.locator(".view-only-notebook")).toBeVisible({
    timeout: 30_000,
  });
  await page.waitForTimeout(3_000); // hydration (suite convention)

  expect(runs, `${path} ran a cell on load`).toEqual([]);
  await expect(page.locator(".view-only-cell-status--running")).toHaveCount(0);

  // Positive control: the same page runs when the reader asks.
  const ran = page.waitForRequest((req) => isRunTraffic(req.url()), {
    timeout: 30_000,
  });
  await page.locator(".run-all-button").click();
  await ran;

  await context.close();
}

/** Import the reactive fixture as a local notebook and open its editor. */
async function openReactiveNotebook(page: Page): Promise<void> {
  await page.goto("/");
  await expect(page.locator(".ironpad-home")).toBeVisible({ timeout: 15_000 });
  await page.waitForTimeout(3_000); // hydration (suite convention)
  const imported = await page.evaluate(
    (json) => window.IronpadStorage.importNotebook(json),
    JSON.stringify(REACTIVE_FIXTURE),
  );
  await page.goto(`/local/${imported.id}`);
  await expect(page.locator(".ironpad-editor")).toBeVisible({
    timeout: 15_000,
  });
}

test.describe("Reactive notebooks respect the autorun boundary", () => {
  test("shared and mutable readers and their embeds never run a reactive notebook on load", async ({
    page,
    browser,
  }) => {
    test.setTimeout(240_000);

    // The author: an immutable share, then a published mutable copy (Share
    // Mutable moves the local notebook into the account, so it goes second).
    await loginTestUser(page, "autorun-author");
    await openReactiveNotebook(page);

    await menuClick(page, "Share Immutable");
    const toast = page.locator(".ironpad-toast-body", { hasText: "/shared/" });
    await expect(toast).toBeVisible({ timeout: 60_000 });
    const hash = (await toast.textContent())!.match(/\/shared\/([0-9a-f]{16})/)![1];

    const id = await shareMutable(page);

    await expectIdleOnLoad(browser, `/shared/${hash}`);
    await expectIdleOnLoad(browser, `/embed/shared/${hash}`);
    await expectIdleOnLoad(browser, `/mutable/${id}`);
    await expectIdleOnLoad(browser, `/embed/mutable/${id}`);
  });

  test("a reactive public notebook's embed waits for the embedder's opt-in", async ({
    browser,
  }) => {
    test.setTimeout(120_000);
    // reactive-demo ships with reactive_mode on. Its /public page runs on
    // load (the route grants it); its embed is click-to-run like any other
    // until the embedder adds `data-autorun`.
    await expectIdleOnLoad(browser, "/embed/public/reactive-demo.ironpad");
  });
});
