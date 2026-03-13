/**
 * Helpers for starting/stopping agent sessions in Playwright tests.
 */
import { Page, expect } from "@playwright/test";

/** Start an agent session and return the token. */
export async function startSession(page: Page): Promise<string> {
  // Click the session button.
  const btn = page.locator(".ironpad-session-button");
  await expect(btn).toBeVisible({ timeout: 5_000 });
  await btn.click();

  // Wait for the session panel to appear with a token.
  const tokenEl = page.locator(".ironpad-session-token");
  await expect(tokenEl).toBeVisible({ timeout: 10_000 });

  // Wait for the token to be populated (not just asterisks).
  // Click "Show" to reveal the token.
  const showBtn = page.locator(".ironpad-session-token-toggle");
  await showBtn.click();

  // Read the token text.
  const token = await tokenEl.textContent();
  expect(token).toBeTruthy();
  expect(token!.length).toBe(64);

  return token!;
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
