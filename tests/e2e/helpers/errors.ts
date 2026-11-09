/**
 * Shared page-error tracking for Playwright tests.
 */
import { Page } from "@playwright/test";

/**
 * A panic in the app's own wasm, as `console_error_panic_hook` reports it.
 *
 * An app panic never reaches `pageerror` as itself: the hook logs the message
 * through `console.error`, and the trap that follows is an "unreachable"
 * RuntimeError, which {@link trackJsErrors} drops as a cell panic. So it is
 * matched on the console instead, by the crate in the panic's location. That
 * has to be an allowlist, because Playwright forwards worker console messages
 * to the page and a cell's panic prints the same "panicked at" line, located
 * in `src/lib.rs` or in one of the cell's dependencies. `wasm-bindgen-futures`
 * is left out for that reason: async cells link it too.
 */
const APP_PANIC =
  /panicked at \S*(ironpad[-_]app|ironpad[-_]common|ironpad[-_]frontend|reactive_graph|reactive_stores|leptos|tachys)|already been disposed|already borrowed/;

/**
 * Start collecting the errors a spec should fail on, so
 * `expect(errors).toEqual([])` stays meaningful:
 *   - uncaught page errors, minus two known-benign classes:
 *     - "unreachable": the WASM trap a panicking cell throws (surfaced
 *       separately as an inline compile/run error in the UI). An app panic
 *       ends in the same trap, which is what the console arm below is for.
 *     - "Canceled": Monaco's cancellation-token rejection on rapid
 *       edits/teardown.
 *   - console errors reporting a panic in the app's own wasm
 *     ({@link APP_PANIC}), such as a disposed-signal read, which aborts the
 *     whole app while the DOM it already rendered stays on screen.
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
  page.on("console", (msg) => {
    if (msg.type() === "error" && APP_PANIC.test(msg.text())) {
      errors.push(msg.text());
    }
  });
  return errors;
}
