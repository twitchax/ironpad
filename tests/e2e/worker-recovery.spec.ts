import { test, expect } from "@playwright/test";

/**
 * Executor-bridge worker lifecycle (fanout-review wave 2). An uncaught
 * worker error does NOT stop the worker per the HTML spec, so onerror must
 * terminate the old one before respawning (or it leaks and keeps posting
 * stale messages), and rapid respawns must cap out to the main-thread
 * fallback instead of tight-looping after a bad deploy. Driven with
 * synthetic ErrorEvents: assigning `.onerror` registers a listener, so
 * `dispatchEvent(new ErrorEvent("error"))` exercises the real handler.
 */

test.describe("Executor worker recovery", () => {
  test("a crashed worker respawns, rapid crashes cap out, terminate recovers", async ({
    page,
  }) => {
    test.setTimeout(60_000);
    await page.goto("/");
    await expect(page.locator(".ironpad-home")).toBeVisible({
      timeout: 15_000,
    });
    await page.waitForTimeout(3_000); // hydration (suite convention)

    const result = await page.evaluate(async () => {
      /* eslint-disable @typescript-eslint/no-explicit-any */
      const exec = (window as any).IronpadExecutor;
      if (!exec || !exec._worker) {
        return { precondition: false };
      }
      const crash = () =>
        exec._worker.dispatchEvent(
          new ErrorEvent("error", { message: "synthetic crash" }),
        );

      // One crash: the old worker is replaced (not merely re-wrapped).
      const first = exec._worker;
      crash();
      const respawned = exec._worker !== null && exec._worker !== first;

      // Two more in quick succession: the respawn cap trips and the bridge
      // parks on the main-thread fallback instead of spawn-looping.
      crash();
      crash();
      const cappedOut = exec._worker === null;

      // With no worker, a request rejects cleanly (the execute path's catch
      // is what routes callers to the fallback executor).
      let rejected = false;
      try {
        await exec._postRequest({ type: "isLoaded", cellId: "nope" });
      } catch (e) {
        rejected = /Worker unavailable/.test(String(e));
      }

      // A deliberate terminate() is a fresh start: worker back, counter reset.
      exec.terminate();
      const recovered = exec._worker !== null;

      return { precondition: true, respawned, cappedOut, rejected, recovered };
    });

    expect(result).toEqual({
      precondition: true,
      respawned: true,
      cappedOut: true,
      rejected: true,
      recovered: true,
    });
  });
});
