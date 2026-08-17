import { test, expect, Page } from "@playwright/test";
import { trackJsErrors } from "./../helpers/errors";

/**
 * PRD-0066 uat-002 and uat-003, driven against the SHIPPED public notebook.
 *
 * These boot real pods and so run only under `cargo make test-linux-cells`
 * (see the README beside this file). Three tests, three pods, ~30 tokens.
 *
 * **Why it drives `/public/linux-cells` instead of authoring cells inline.**
 * The notebook is the artifact readers reach, and the claims under test are
 * claims about it. A spec that typed its own programs would prove the runtime
 * works while the shipped document rotted independently — and nothing else
 * would notice, because Linux cells carry no `saved_output`, so the
 * `capture-outputs` freshness gate that covers every other notebook is blind
 * to this one by construction.
 */

const NOTEBOOK = "/public/linux-cells";

/** Linux cells in notebook order: proc, write, read, pipeline, threads. */
const Cell = {
  Proc: 0,
  Write: 1,
  Read: 2,
  Pipeline: 3,
  Threads: 4,
} as const;

const cellAt = (page: Page, i: number) =>
  page.locator(".view-only-cell--linux").nth(i);

/** Run one Linux cell and wait for the shell sentinel to report a status. */
async function runCell(page: Page, i: number, timeout = 600_000) {
  const cell = cellAt(page, i);
  const run = cell.locator(".view-only-run-button");
  await expect(run).toBeEnabled({ timeout: 60_000 });
  await run.click();
  await expect(cell.locator(".ironpad-linux-status")).toContainText(
    /exit|finished/,
    { timeout },
  );
  return {
    text:
      (await cell.locator(".ironpad-linux-terminal-text").textContent()) ?? "",
    status: (await cell.locator(".ironpad-linux-status").textContent()) ?? "",
  };
}

async function openNotebook(page: Page) {
  await page.goto(NOTEBOOK);
  await expect(page.locator(".view-only-notebook")).toBeVisible({
    timeout: 30_000,
  });
  await expect(page.locator(".view-only-cell--linux")).toHaveCount(5);
  // A public notebook AUTORUNS. Five Linux cells sitting in one and no status
  // anywhere is uat-005 restated on the page that would actually pay for it.
  await page.waitForTimeout(5_000);
  await expect(page.locator(".ironpad-linux-status")).toHaveCount(0);
}

test("a Linux cell streams a real program's stdout (uat-002)", async ({
  page,
}) => {
  test.setTimeout(900_000);
  const jsErrors = trackJsErrors(page);

  await openNotebook(page);
  const { text, status } = await runCell(page, Cell.Proc);

  // Assert things only a real process can report, so this measures the
  // runtime rather than the panel.
  expect(status).toContain("exit 0");
  expect(text).toMatch(/^pid\s+\d+/m);
  expect(text).toMatch(/target\s+wasm64-linux/);
  expect(text).toMatch(/\/proc\/self\s+.*\bstatus\b/);
  expect(text).toMatch(/\/bin\s+\d{3} executables/);
  expect(jsErrors).toEqual([]);
});

test("two Linux cells share one filesystem (uat-003)", async ({ page }) => {
  test.setTimeout(900_000);
  const jsErrors = trackJsErrors(page);

  await openNotebook(page);

  const writer = await runCell(page, Cell.Write);
  expect(writer.status).toContain("exit 0");
  const wrote = writer.text.match(/pid (\d+) wrote .*\((\d+) bytes\)/);
  expect(wrote, `writer said: ${writer.text}`).not.toBeNull();

  const reader = await runCell(page, Cell.Read);
  expect(reader.status).toContain("exit 0");
  const read = reader.text.match(/pid (\d+) is a different process/);
  expect(read, `reader said: ${reader.text}`).not.toBeNull();

  // The load-bearing half: a SECOND process saw the first one's bytes, with
  // no piping between them. Distinct pids is what makes it two processes
  // rather than one program printing twice.
  expect(read![1]).not.toBe(wrote![1]);
  for (const station of ["cairo", "oslo", "quito"]) {
    expect(reader.text).toContain(station);
  }
  expect(reader.text).toMatch(/n=24/);
  expect(jsErrors).toEqual([]);
});

test("subprocesses and threads are real (T-013)", async ({ page }) => {
  test.setTimeout(900_000);
  const jsErrors = trackJsErrors(page);

  await openNotebook(page);

  // The pipeline reads what the writer leaves behind, so it runs first.
  await runCell(page, Cell.Write);

  const pipe = await runCell(page, Cell.Pipeline);
  expect(pipe.status).toContain("exit 0");
  // `uniq -c` output: 24 rows per station, counted by three chained
  // processes that this program never copied bytes through.
  expect(pipe.text).toMatch(/24 cairo/);
  expect(pipe.text).toMatch(/24 oslo/);
  expect(pipe.text).toMatch(/24 quito/);
  expect(pipe.text).toMatch(/cut\s+exit status: 0/);
  expect(pipe.text).toMatch(/sort\s+exit status: 0/);

  const threads = await runCell(page, Cell.Threads);
  expect(threads.status).toContain("exit 0");

  // Printed, not just asserted. The notebook's prose quotes a serial and a
  // 16-thread number, and those are host-dependent claims that can only be
  // kept honest by someone reading what this run actually measured.
  console.log(`\n--- threads cell, as measured ---\n${threads.text}`);

  // `available_parallelism()` reports 1 in a pod. The notebook says so, and a
  // pod that started reporting a real count would make that prose wrong.
  expect(threads.text).toMatch(/available_parallelism\s+Ok\(1\)/);

  // Four threads each observing the other three: threads taking turns cannot
  // produce this, so it is the assertion that distinguishes real concurrency
  // from interleaving.
  expect(threads.text).toMatch(/alive at once\s+4 of 4/);

  // And the fan-out has to actually pay. Deliberately loose: the factor
  // depends on the host, and an exact number here would be a flake.
  const best = [
    ...threads.text.matchAll(/(\d+)\s+threads\s+\d+ ms\s+([\d.]+)x/g),
  ].map((m) => Number(m[2]));
  expect(best.length, `no speedup rows in: ${threads.text}`).toBeGreaterThan(0);
  expect(Math.max(...best)).toBeGreaterThan(1.5);
  expect(jsErrors).toEqual([]);
});
