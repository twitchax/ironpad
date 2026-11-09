/**
 * Hold `compile_cell` requests at the network layer so a spec can act while a
 * cell is provably mid-compile, then let them through in an order it picks.
 *
 * The route handler runs in this Node process, so a spec that installs a gate
 * must not block the event loop afterwards (no `execFileSync`, which is what
 * `cliExec` uses): a held request can only be continued, and a passthrough
 * request only forwarded, while the loop is free. Use `cliExecRawAsync`.
 */
import { Page, Route, expect } from "@playwright/test";

export interface CompileGate {
  /** Resolve once at least `n` compile requests have arrived and are held. */
  waitForHeld(n: number, timeoutMs?: number): Promise<void>;
  /** Let the `i`-th held request (arrival order) through. */
  release(i: number): void;
  /** Let every held request through, and every later one straight past. */
  releaseAll(): void;
  /** How many compile requests the gate has held so far. */
  held(): number;
}

export async function gateCompiles(page: Page): Promise<CompileGate> {
  const held: { route: Route; release: () => void }[] = [];
  let passthrough = false;
  await page.route(
    (url) => url.pathname.includes("compile_cell"),
    async (route) => {
      if (!passthrough) {
        await new Promise<void>((resolve) => held.push({ route, release: resolve }));
      }
      await route.continue();
    },
  );
  return {
    async waitForHeld(n, timeoutMs = 60_000) {
      await expect.poll(() => held.length, { timeout: timeoutMs }).toBeGreaterThanOrEqual(n);
    },
    release(i) {
      held[i].release();
    },
    releaseAll() {
      passthrough = true;
      held.forEach((h) => h.release());
    },
    held: () => held.length,
  };
}
