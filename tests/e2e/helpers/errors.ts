/**
 * Shared page-error tracking for Playwright tests.
 */
import { Page } from "@playwright/test";

/**
 * Start collecting uncaught page errors, dropping the two known-benign classes
 * so `expect(errors).toEqual([])` stays meaningful:
 *   - "unreachable": the WASM trap a panicking cell throws (surfaced separately
 *     as an inline compile/run error in the UI).
 *   - "Canceled": Monaco's cancellation-token rejection on rapid edits/teardown.
 *
 * Returns the live array — assert on it at the end of the test.
 */
export function trackJsErrors(page: Page): string[] {
  const errors: string[] = [];
  page.on("pageerror", (error) => {
    if (
      !error.message.includes("unreachable") &&
      !error.message.includes("Canceled")
    ) {
      errors.push(error.message);
    }
  });
  return errors;
}
