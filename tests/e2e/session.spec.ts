import { test, expect } from "@playwright/test";
import {
  createNotebook,
  startSession,
  endSession,
  waitForPersistedCells,
} from "./helpers/session";
import { connectCli, cliExec, cliExecRaw, stopCli, CliHandle } from "./helpers/cli";
import { trackJsErrors } from "./helpers/errors";

// Session tests share a single daemon socket (under a per-run temp HOME; see
// helpers/cli.ts) and must run serially to avoid conflicts.
test.describe.serial("Agent Session", () => {
  let cliHandle: CliHandle | null = null;

  test.afterEach(async () => {
    if (cliHandle) {
      stopCli(cliHandle);
      cliHandle = null;
    }
  });

  test("session lifecycle: start, connect, end", async ({ page }) => {
    test.setTimeout(60_000);

    const jsErrors = trackJsErrors(page);

    // Create a new notebook.
    await createNotebook(page);

    // Start session and get token.
    const token = await startSession(page);
    expect(token).toHaveLength(64);

    // Connect CLI daemon.
    cliHandle = await connectCli(token);

    // Verify daemon reports connected.
    const status = cliExec(["status"]);
    expect(status.connected).toBe(true);
    expect(status.cached).toBe(true);

    // Verify browser shows agent connected.
    await expect(
      page.locator(".ironpad-session-button")
    ).toContainText(/agent/, { timeout: 5_000 });

    // End session from browser.
    await endSession(page);

    // Verify CLI commands fail after the session ends. Poll rather than sleep:
    // the daemon needs a moment to observe the WS close, and `status` then
    // exits non-zero.
    await expect
      .poll(() => cliExecRaw(["status"]).exitCode, { timeout: 10_000 })
      .not.toBe(0);

    expect(jsErrors).toEqual([]);
  });

  test("agent reads notebook cells", async ({ page }) => {
    test.setTimeout(60_000);

    // Create notebook with a cell.
    await createNotebook(page);

    // Add a cell via UI.
    await page.locator(".ironpad-add-cell-btn").first().click();
    await expect(page.locator(".ironpad-cell-card")).toHaveCount(1);

    // Wait for the cell to actually land in IndexedDB (persistence is
    // fire-and-forget; a fixed sleep raced it).
    await waitForPersistedCells(page, 1);

    // Start session and connect CLI.
    const token = await startSession(page);
    cliHandle = await connectCli(token);

    // List cells.
    const cells = cliExec(["cells", "list"]);
    expect(cells).toHaveLength(1);
    expect(cells[0].label).toBeTruthy();
    expect(cells[0].cell_type).toBe("Code");

    // Get cell details.
    const cell = cliExec(["cells", "get", cells[0].id]);
    expect(cell.source).toBeTruthy();
    expect(cell.id).toBe(cells[0].id);
  });

  test("agent adds and deletes cells", async ({ page }) => {
    test.setTimeout(60_000);

    // Create empty notebook.
    await createNotebook(page);

    // Start session and connect.
    const token = await startSession(page);
    cliHandle = await connectCli(token);

    // Add a cell via CLI.
    const addResult = cliExecRaw([
      "cells",
      "add",
      "--source",
      "let x = 42;",
      "--label",
      "Agent Cell",
    ]);
    expect(addResult.exitCode).toBe(0);

    // Verify browser shows the new cell.
    await expect(page.locator(".ironpad-cell-card")).toHaveCount(1, {
      timeout: 15_000,
    });

    // List and verify.
    const cells = cliExec(["cells", "list"]);
    expect(cells).toHaveLength(1);

    // Delete the cell.
    const deleteResult = cliExecRaw(["cells", "delete", cells[0].id]);
    expect(deleteResult.exitCode).toBe(0);

    // Verify browser shows no cells.
    await expect(page.locator(".ironpad-cell-card")).toHaveCount(0, {
      timeout: 15_000,
    });
  });

  test("agent updates cell source", async ({ page }) => {
    test.setTimeout(60_000);

    // Create notebook with a cell.
    await createNotebook(page);
    await page.locator(".ironpad-add-cell-btn").first().click();
    await expect(page.locator(".ironpad-cell-card")).toHaveCount(1);
    await waitForPersistedCells(page, 1);

    // Start session and connect.
    const token = await startSession(page);
    cliHandle = await connectCli(token);

    // Get cell ID.
    const cells = cliExec(["cells", "list"]);
    const cellId = cells[0].id;

    // Update source via CLI.
    const updateResult = cliExecRaw([
      "cells",
      "update",
      cellId,
      "--source",
      "let x = 99;",
    ]);
    expect(updateResult.exitCode).toBe(0);

    // Verify the cell source was updated in the daemon cache.
    const cell = cliExec(["cells", "get", cellId]);
    expect(cell.source).toContain("let x = 99;");
  });

  test("agent runs a cell and reads its output (PRD-0052)", async ({
    page,
  }) => {
    // Compile-scale timeout: the run blocks on a real (possibly cold) build.
    test.setTimeout(240_000);

    const jsErrors = trackJsErrors(page);

    await createNotebook(page);
    const token = await startSession(page);
    cliHandle = await connectCli(token);

    // Author the cell from the agent side too — the full loop is write,
    // run, read. Same source as execution.spec.ts, so the compile cache is
    // usually warm.
    const addResult = cliExecRaw([
      "cells",
      "add",
      "--source",
      'CellOutput::text(format!("{}", 42))',
      "--label",
      "Agent Run",
    ]);
    expect(addResult.exitCode).toBe(0);
    await expect(page.locator(".ironpad-cell-card")).toHaveCount(1, {
      timeout: 15_000,
    });

    const cells = cliExec(["cells", "list"]);
    const cellId = cells[0].id;

    // Run and block until the browser reports the terminal event.
    const result = cliExec(["cells", "run", cellId], 180_000);
    expect(result.status).toBe("executed");
    expect(result.cell_id).toBe(cellId);
    expect(result.display_text).toContain("42");
    expect(result.execution_time_ms).toBeGreaterThanOrEqual(0);

    // The browser really executed it: success badge + rendered output — the
    // CLI result and the page must agree.
    const cell = page.locator(".ironpad-cell-card").first();
    await expect(cell.locator(".ironpad-cell-status--success")).toBeVisible({
      timeout: 10_000,
    });
    await expect(cell.locator(".ironpad-output-display-text")).toContainText(
      "42"
    );

    // A cell that doesn't exist is refused up front, not queued into a
    // timeout.
    const bogus = cliExecRaw(["cells", "run", "no-such-cell"]);
    expect(bogus.exitCode).not.toBe(0);

    expect(jsErrors).toEqual([]);
  });

  test("navigating away ends the session instead of poisoning the page", async ({
    page,
  }) => {
    test.setTimeout(90_000);

    const jsErrors = trackJsErrors(page);
    // The regression signature: a leaked WS handler reading the disposed
    // page's signals panics with "already been disposed" (a console.error,
    // not a pageerror — and the follow-on trap is "unreachable", which
    // trackJsErrors treats as a benign cell panic). Watch for it directly.
    const disposedPanics: string[] = [];
    page.on("console", (msg) => {
      if (
        msg.type() === "error" &&
        (msg.text().includes("already been disposed") ||
          msg.text().includes("RefCell already borrowed"))
      ) {
        disposedPanics.push(msg.text());
      }
    });

    await createNotebook(page);
    const notebookId = page.url().match(/\/local\/([a-f0-9-]+)/)![1];
    const token = await startSession(page);
    cliHandle = await connectCli(token);
    expect(cliExec(["status"]).connected).toBe(true);

    // Leave the notebook WITHOUT stopping the session. Pre-fix, the socket
    // and its forget()-leaked handlers survived the page; the next incoming
    // agent message then panicked the wasm runtime and left every page
    // render-only until a hard reload ("cannot edit the title").
    await page.locator("a.ironpad-brand").click();
    await expect(page.locator(".ironpad-home")).toBeVisible({
      timeout: 10_000,
    });

    // Page disposal must end the session server-side: the daemon observes
    // the close and commands fail fast instead of stranding on a timeout.
    await expect
      .poll(() => cliExecRaw(["status"]).exitCode, { timeout: 10_000 })
      .not.toBe(0);
    const late = cliExecRaw([
      "cells",
      "add",
      "--label",
      "probe",
      "--source",
      "42",
    ]);
    expect(late.exitCode).not.toBe(0);

    // The SAME wasm instance must still be fully reactive: SPA-navigate back
    // into the notebook (no reload — a reload would mask a poisoned runtime)
    // and run the title-edit flow end to end.
    await page.locator(`a[href="/local/${notebookId}"]`).click();
    const title = page.locator(".ironpad-notebook-title--editable");
    await expect(title).toBeVisible({ timeout: 15_000 });
    await title.click();
    const input = page.locator(".ironpad-header-title-input");
    await expect(input).toBeVisible();
    await input.fill("");
    await input.pressSequentially("Still Alive", { delay: 20 });
    await input.press("Enter");
    await expect(title).toHaveText("Still Alive", { timeout: 10_000 });

    expect(disposedPanics).toEqual([]);
    expect(jsErrors).toEqual([]);
  });

  test("session ends on tab close", async ({ page }) => {
    test.setTimeout(60_000);

    // Create notebook.
    await createNotebook(page);

    // Start session and connect.
    const token = await startSession(page);
    cliHandle = await connectCli(token);

    // Verify connected.
    const status = cliExec(["status"]);
    expect(status.connected).toBe(true);

    // Close the browser tab.
    await page.close();

    // Give daemon time to detect disconnect.
    await new Promise((r) => setTimeout(r, 3_000));

    // Verify CLI commands fail.
    const result = cliExecRaw(["status"]);
    expect(result.exitCode).not.toBe(0);
  });
});
