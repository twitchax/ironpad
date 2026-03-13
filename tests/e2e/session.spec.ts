import { test, expect } from "@playwright/test";
import { startSession, endSession } from "./helpers/session";
import { connectCli, cliExec, cliExecRaw, stopCli, CliHandle } from "./helpers/cli";

test.describe("Agent Session", () => {
  let cliHandle: CliHandle | null = null;

  test.afterEach(async () => {
    if (cliHandle) {
      stopCli(cliHandle);
      cliHandle = null;
    }
  });

  test("session lifecycle: start, connect, end", async ({ page }) => {
    test.setTimeout(60_000);

    const jsErrors: string[] = [];
    page.on("pageerror", (error) => {
      if (!error.message.includes("unreachable")) {
        jsErrors.push(error.message);
      }
    });

    // Create a new notebook.
    await page.goto("/");
    await page.locator("button", { hasText: "+ New Notebook" }).click();
    await expect(page).toHaveURL(/\/notebook\/[a-f0-9-]+/);
    await expect(page.locator(".ironpad-editor")).toBeVisible();

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

    // Verify CLI commands fail after session ends.
    // Give the daemon a moment to receive the close.
    await page.waitForTimeout(2_000);
    const result = cliExecRaw(["status"]);
    expect(result.exitCode).not.toBe(0);

    expect(jsErrors).toEqual([]);
  });

  test("agent reads notebook cells", async ({ page }) => {
    test.setTimeout(60_000);

    // Create notebook with a cell.
    await page.goto("/");
    await page.locator("button", { hasText: "+ New Notebook" }).click();
    await expect(page.locator(".ironpad-editor")).toBeVisible();

    // Add a cell via UI.
    await page.locator(".ironpad-add-cell-btn").first().click();
    await expect(page.locator(".ironpad-cell-card")).toHaveCount(1);

    // Wait for cell to persist.
    await page.waitForTimeout(2_000);

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
    await page.goto("/");
    await page.locator("button", { hasText: "+ New Notebook" }).click();
    await expect(page.locator(".ironpad-editor")).toBeVisible();

    // Start session and connect.
    const token = await startSession(page);
    cliHandle = await connectCli(token);

    // Add a cell via CLI.
    const addResult = cliExecRaw([
      "cells",
      "add",
      "--source",
      '"let x = 42;"',
      "--label",
      '"Agent Cell"',
    ]);
    expect(addResult.exitCode).toBe(0);

    // Verify browser shows the new cell.
    await expect(page.locator(".ironpad-cell-card")).toHaveCount(1, {
      timeout: 5_000,
    });

    // List and verify.
    const cells = cliExec(["cells", "list"]);
    expect(cells).toHaveLength(1);

    // Delete the cell.
    const deleteResult = cliExecRaw(["cells", "delete", cells[0].id]);
    expect(deleteResult.exitCode).toBe(0);

    // Verify browser shows no cells.
    await expect(page.locator(".ironpad-cell-card")).toHaveCount(0, {
      timeout: 5_000,
    });
  });

  test("agent updates cell source", async ({ page }) => {
    test.setTimeout(60_000);

    // Create notebook with a cell.
    await page.goto("/");
    await page.locator("button", { hasText: "+ New Notebook" }).click();
    await expect(page.locator(".ironpad-editor")).toBeVisible();
    await page.locator(".ironpad-add-cell-btn").first().click();
    await expect(page.locator(".ironpad-cell-card")).toHaveCount(1);
    await page.waitForTimeout(2_000);

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
      '"let x = 99;"',
    ]);
    expect(updateResult.exitCode).toBe(0);

    // Verify the cell source was updated in the daemon cache.
    const cell = cliExec(["cells", "get", cellId]);
    expect(cell.source).toContain("let x = 99;");
  });

  test("session ends on tab close", async ({ page }) => {
    test.setTimeout(60_000);

    // Create notebook.
    await page.goto("/");
    await page.locator("button", { hasText: "+ New Notebook" }).click();
    await expect(page.locator(".ironpad-editor")).toBeVisible();

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
