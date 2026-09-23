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

// ── A hand-assembled tick module ────────────────────────────────────────────
//
// The smallest WASM a tick needs: memory, ironpad_alloc (always 1024),
// a no-op ironpad_dealloc, and a zero-arg cell_tick returning 16, where a
// data segment holds `data`. No compile, no cache, and the raw loading path
// the main-thread fallback takes for a blob without JS glue.

function tickModule(data: number[]): number[] {
  const leb = (n: number): number[] => {
    const out: number[] = [];
    do {
      let b = n & 0x7f;
      n >>>= 7;
      if (n !== 0) b |= 0x80;
      out.push(b);
    } while (n !== 0);
    return out;
  };
  const str = (s: string) => [...leb(s.length), ...Array.from(s, (c) => c.charCodeAt(0))];
  const vec = (items: number[][]) => [...leb(items.length), ...items.flat()];
  const section = (id: number, body: number[]) => [id, ...leb(body.length), ...body];
  const body = (code: number[]) => [...leb(code.length + 1), 0, ...code]; // no locals
  const I32 = 0x7f;
  return [
    0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00,
    ...section(1, vec([
      [0x60, 1, I32, 1, I32], // (i32) -> i32: ironpad_alloc
      [0x60, 2, I32, I32, 0], // (i32, i32):   ironpad_dealloc
      [0x60, 0, 1, I32], //      () -> i32:    cell_tick
    ])),
    ...section(3, vec([[0], [1], [2]])),
    ...section(5, vec([[0x00, 1]])), // one memory, one page
    ...section(7, vec([
      [...str("memory"), 0x02, 0],
      [...str("ironpad_alloc"), 0x00, 0],
      [...str("ironpad_dealloc"), 0x00, 1],
      [...str("cell_tick"), 0x00, 2],
    ])),
    ...section(10, vec([
      body([0x41, 0x80, 0x08, 0x0b]), // i32.const 1024
      body([0x0b]),
      body([0x41, 16, 0x0b]), //         i32.const 16
    ])),
    ...section(11, vec([[0x00, 0x41, 16, 0x0b, ...leb(data.length), ...data]])),
  ];
}

const u32 = (n: number) => [n & 0xff, (n >>> 8) & 0xff, (n >>> 16) & 0xff, (n >>> 24) & 0xff];

// TickResult at 16: rgb at 48, 3 bytes, a 1x1 frame; the RGB bytes at 48.
const SIM_TICK = tickModule([
  ...u32(48), ...u32(3), ...u32(1), ...u32(1), ...new Array(16).fill(0), 10, 20, 30,
]);
// LiveTickResult at 16: kind 0 (Text), content at 48, 2 bytes: "hi".
const LIVE_TICK = tickModule([
  ...u32(0), ...u32(48), ...u32(2), ...new Array(20).fill(0), 104, 105,
]);

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

  test("with the worker parked, tick and tickLive run on the main-thread fallback", async ({
    page,
  }) => {
    // execute() is not the only caller of the fallback policy (review js-3):
    // loadBlob, tick and tickLive share it, and a Simulation or LiveView cell
    // reaches it every frame once the worker is gone.
    test.setTimeout(60_000);
    await page.goto("/");
    await expect(page.locator(".ironpad-home")).toBeVisible({
      timeout: 15_000,
    });
    await page.waitForTimeout(3_000); // hydration (suite convention)

    const result = await page.evaluate(
      async ([sim, live]) => {
        /* eslint-disable @typescript-eslint/no-explicit-any */
        const exec = (window as any).IronpadExecutor;
        if (!exec || !exec._worker) {
          return { precondition: false };
        }
        const crash = () =>
          exec._worker.dispatchEvent(
            new ErrorEvent("error", { message: "synthetic crash" }),
          );
        crash();
        crash();
        crash();
        const parked = exec._worker === null;

        // loadBlob falls back too, and must not claim the WORKER loaded it.
        await exec.loadBlob("e2e-sim", "h-sim", new Uint8Array(sim), null);
        await exec.loadBlob("e2e-live", "h-live", new Uint8Array(live), null);
        const workerCacheUntouched =
          !exec.isLoaded("e2e-sim", "h-sim") && !exec.isLoaded("e2e-live", "h-live");

        const frame = await exec.tick("e2e-sim");
        const view = await exec.tickLive("e2e-live");

        exec.terminate();
        return {
          precondition: true,
          parked,
          workerCacheUntouched,
          frame: {
            fallback: frame.fallback,
            width: frame.width,
            height: frame.height,
            rgb: Array.from(frame.rgbBytes as Uint8Array),
          },
          view: { fallback: view.fallback, kind: view.kind, content: view.content },
        };
      },
      [SIM_TICK, LIVE_TICK] as const,
    );

    expect(result).toEqual({
      precondition: true,
      parked: true,
      workerCacheUntouched: true,
      frame: { fallback: true, width: 1, height: 1, rgb: [10, 20, 30] },
      view: { fallback: true, kind: 0, content: "hi" },
    });
  });
});
