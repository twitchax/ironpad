/**
 * Helpers for starting/stopping agent sessions in Playwright tests.
 */
import { Page, expect } from "@playwright/test";

/**
 * Navigate home and create a fresh notebook, robustly under full-suite load:
 * wait for the home page AND hydration before clicking (a pre-hydration click
 * hits no handler and strands the test on the home page), and give the editor
 * a load-tolerant timeout.
 */
export async function createNotebook(page: Page): Promise<void> {
  await page.goto("/");
  await expect(page.locator(".ironpad-home")).toBeVisible({ timeout: 15_000 });
  await page.waitForTimeout(3_000); // hydration (suite convention)
  await page.locator("button", { hasText: "+ New Notebook" }).click();
  await expect(page).toHaveURL(/\/notebook\/[a-f0-9-]+/, { timeout: 15_000 });
  await expect(page.locator(".ironpad-editor")).toBeVisible({
    timeout: 15_000,
  });
}

/** Start an agent session and return the token. */
export async function startSession(page: Page): Promise<string> {
  // Click the session button.
  const btn = page.locator(".ironpad-session-button");
  await expect(btn).toBeVisible({ timeout: 15_000 });
  await btn.click();

  // Wait for the session panel to appear with a token.
  const tokenEl = page.locator(".ironpad-session-token");
  await expect(tokenEl).toBeVisible({ timeout: 10_000 });

  // Click "Show" to reveal the token, then WAIT for the real value — the
  // reveal is async, and reading immediately raced it (the trailing length
  // assertion only caught the race after the fact).
  const showBtn = page.locator(".ironpad-session-token-toggle");
  await showBtn.click();
  await expect(tokenEl).toHaveText(/^[0-9a-f]{64}$/, { timeout: 10_000 });

  const token = await tokenEl.textContent();
  expect(token).toBeTruthy();
  expect(token!.length).toBe(64);

  return token!;
}

/**
 * Poll IndexedDB until the current notebook has `count` persisted cells.
 *
 * Deterministic replacement for fixed "wait for cell to persist" sleeps:
 * persistence is fire-and-forget (persist_notebook), so only the durable
 * store says when it has actually landed.
 */
export async function waitForPersistedCells(
  page: Page,
  count: number
): Promise<void> {
  const notebookId = page.url().match(/\/notebook\/([a-f0-9-]+)/)![1];
  await expect
    .poll(
      () =>
        page.evaluate(async (id) => {
          const nb = await (window as any).IronpadStorage.getNotebook(id);
          return nb?.cells?.length ?? 0;
        }, notebookId),
      { timeout: 10_000 }
    )
    .toBe(count);
}

/** End the active session. */
export async function endSession(page: Page): Promise<void> {
  // The panel should be open or we need to open it.
  const panel = page.locator(".ironpad-session-panel");
  if (!(await panel.isVisible())) {
    const btn = page.locator(".ironpad-session-button");
    await btn.click();
  }

  // Click "End Session".
  await page.locator("button", { hasText: "End Session" }).click();

  // Verify the session button returns to inactive state.
  await expect(page.locator(".ironpad-session-button--active")).toBeHidden({
    timeout: 5_000,
  });
}

/** Get the number of connected agents shown in the session panel. */
export async function getConnectedAgentCount(page: Page): Promise<number> {
  const panel = page.locator(".ironpad-session-panel");
  if (!(await panel.isVisible())) {
    const btn = page.locator(".ironpad-session-button");
    await btn.click();
  }

  const agents = page.locator(".ironpad-session-agent-list .thaw-tag");
  return agents.count();
}
