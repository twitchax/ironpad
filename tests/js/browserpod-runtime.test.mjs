/**
 * `IronpadPod` unit tests (PRD-0066).
 *
 *   node --test tests/js/
 *
 * These boot NO pod and contact no network. They load the real
 * `public/browserpod-runtime.js` and drive its public API against a fake
 * BrowserPod SDK, which is the only way to reach the lifecycle bugs at all: a
 * teardown during a boot, a teardown during the binary write, and a slow boot
 * landing after a fast one are all races between two awaits, invisible to a
 * Playwright spec and unreachable without a pod that costs 10 metered tokens
 * to start.
 *
 * The runtime is loaded as source and patched at exactly one site — its
 * dynamic `import()` of rt.browserpod.io, which Node cannot resolve and which
 * must never be resolved from a test anyway. The patch asserts it matched
 * once, so this fails loudly rather than silently testing nothing if that line
 * moves.
 */

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const RUNTIME_PATH = path.join(HERE, "..", "..", "public", "browserpod-runtime.js");

const IMPORT_SITE = "mod = await import(SDK_URL);";
const TEST_IMPORT = "mod = await window.__testImport(SDK_URL);";

const SOURCE = readFileSync(RUNTIME_PATH, "utf8");
assert.equal(
  SOURCE.split(IMPORT_SITE).length - 1,
  1,
  `expected exactly one \`${IMPORT_SITE}\` in browserpod-runtime.js`,
);
const PATCHED = SOURCE.replace(IMPORT_SITE, TEST_IMPORT);

// Mirrors of the constants under test. Duplicated deliberately: a test that
// read them out of the module could not tell a grace of five minutes from one
// of 250ms, which is the whole point of the assertion that uses them.
const UNKNOWN_AFTER_MS = 300000;
const TEARDOWN_GRACE_MS = 300000;

// ── Harness ─────────────────────────────────────────────────────────────────

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

/** Run pending microtasks. Timers are mocked separately, per test. */
async function flush(rounds = 30) {
  for (let i = 0; i < rounds; i++) {
    await Promise.resolve();
  }
}

/** A promise whose settlement can be inspected without awaiting it. */
function capture(promise) {
  const box = { value: null, error: null, settled: false };
  promise.then(
    (v) => {
      box.value = v;
      box.settled = true;
    },
    (e) => {
      box.error = e;
      box.settled = true;
    },
  );
  return box;
}

/**
 * A fresh `IronpadPod` over a fake SDK.
 *
 * `gateBoot()`/`gateWrite()` queue a deferred that the next boot (or the next
 * `createFile`) blocks on, which is how a test gets to stand inside the window
 * a teardown has to survive.
 */
function makeRuntime() {
  const log = { boots: 0, disposals: 0, runs: [], files: [] };
  const bootGates = [];
  const writeGates = [];
  let emitOutput = null;

  function makePod(label) {
    return {
      createDirectory: async () => {},
      createFile: async (p) => {
        log.files.push(p);
        const gate = writeGates.shift();
        if (gate) await gate.promise;
        return {
          write: async (buffer) => buffer.byteLength,
          close: async () => {},
        };
      },
      createCustomTerminal: async ({ onOutput }) => {
        emitOutput = (text) => onOutput(new TextEncoder().encode(text));
        return { onOutput };
      },
      run: async (exe, args) => {
        log.runs.push({ pod: label, exe, args });
        return 4242;
      },
      destroy: () => {
        log.disposals += 1;
      },
    };
  }

  const window = {
    __testImport: async () => ({
      BrowserPod: {
        boot: async () => {
          const label = ++log.boots;
          const gate = bootGates.shift();
          if (gate) await gate.promise;
          return makePod(label);
        },
      },
    }),
  };
  // eslint-disable-next-line no-new-func
  new Function("window", "self", PATCHED)(window, { crossOriginIsolated: true });

  const api = window.IronpadPod;
  api.configure("test-key");

  return {
    api,
    log,
    gateBoot() {
      const gate = deferred();
      bootGates.push(gate);
      return gate;
    },
    gateWrite() {
      const gate = deferred();
      writeGates.push(gate);
      return gate;
    },
    emit(text) {
      assert.ok(emitOutput, "no terminal yet: the program has not started");
      emitOutput(text);
    },
  };
}

/** The shape the Rust side passes, with the text and stage channels tapped. */
function runOptions(rt, cellId = "c1", notebookId = "n1") {
  const tap = { text: "", stages: [] };
  const promise = rt.api.run({
    notebookId,
    cellId,
    bytes: new Uint8Array([0, 1, 2, 3]),
    onText: (t) => {
      tap.text = t;
    },
    onStage: (s) => {
      tap.stages.push(s);
    },
  });
  tap.run = capture(promise);
  return tap;
}

const EXIT_0 = "\x1e_ironpad_exit:0\x1e";

// ── Tests ───────────────────────────────────────────────────────────────────

test("a run reaches the pod and reports the shell's exit status", async () => {
  // The positive control. Without it every assertion below would also pass
  // against a harness that never started anything at all.
  const rt = makeRuntime();
  const tap = runOptions(rt);
  await flush();

  assert.deepEqual(tap.stages, ["booting", "running"]);
  assert.equal(rt.log.runs.length, 1);
  assert.equal(rt.log.runs[0].exe, "/bin/sh");

  rt.emit("hello\n");
  rt.emit(EXIT_0);
  await flush();

  assert.deepEqual(tap.run.value, { status: "exited", exitCode: 0, inferred: false });
  assert.equal(tap.text, "hello");
  assert.equal(rt.log.boots, 1);
});

test("closing the sink flushes the text it was holding back", async () => {
  // The sentinel scanner holds back everything from a record separator on, in
  // case the next chunk completes it. Nothing fed that tail back on close, so
  // a transcript ending inside one lost its last line outright.
  const rt = makeRuntime();
  const tap = runOptions(rt);
  await flush();

  rt.emit("hello\x1eworld");
  await flush();
  assert.equal(tap.text, "hello", "the tail is held back while more may arrive");

  rt.api.teardown();
  await flush();

  assert.equal(tap.run.value.status, "terminated");
  assert.equal(tap.text, "helloworld", "nothing is coming to complete it, so it is text");
});

test("closing the sink drops a half-arrived escape sequence", async () => {
  // The other half of the same decision: an unterminated CSI is control bytes
  // a log view drops, not text. Printing it would put `[31` in the transcript.
  const rt = makeRuntime();
  const tap = runOptions(rt);
  await flush();

  rt.emit("red \x1b[31");
  await flush();
  rt.api.teardown();
  await flush();

  assert.equal(tap.text, "red ");
});

test("a teardown during the boot settles the run and starts no program", async () => {
  // `state.pending` is empty during a boot, so teardown had nothing to settle:
  // the cell sat on "starting machine" with a dead Stop button until the page
  // was reloaded. And the generation was sampled AFTER the boot await, so
  // every later check compared equal and the program ran anyway, on a pod
  // nothing pointed at any more.
  const rt = makeRuntime();
  const gate = rt.gateBoot();
  const tap = runOptions(rt);
  await flush();
  assert.equal(tap.run.settled, false, "still booting");

  rt.api.teardown();
  await flush();
  assert.deepEqual(tap.run.value, { status: "terminated", exitCode: null, inferred: false });

  gate.resolve();
  await flush();
  assert.deepEqual(rt.log.runs, [], "a torn-down run must never start its program");
  assert.deepEqual(rt.log.files, [], "and must never write into the machine it dropped");
  assert.equal(rt.log.disposals, 1, "the orphaned pod is dropped, not left running");
  assert.equal(rt.api.status().booted, false);
});

test("a teardown during the binary write settles the run", async () => {
  // The write sits between the boot's timeout and the run's own bookkeeping:
  // not covered by either, so a teardown here wedged the cell permanently.
  const rt = makeRuntime();
  const gate = rt.gateWrite();
  const tap = runOptions(rt);
  await flush();
  assert.equal(rt.log.files.length, 1, "the write is in flight");
  assert.equal(tap.run.settled, false);

  rt.api.teardown();
  await flush();
  assert.equal(tap.run.value.status, "terminated");

  gate.resolve();
  await flush();
  assert.deepEqual(rt.log.runs, [], "a write that lands late may not start the program");
});

test("a stale boot does not null the handle to a live one", async () => {
  // Both boot handlers cleared `state.booting` unconditionally, so a slow
  // first boot landing after a teardown nulled the second boot's handle. The
  // next click then started a third pod on top of two orphans.
  const rt = makeRuntime();
  const first = rt.gateBoot();
  const stale = runOptions(rt, "c1");
  await flush();
  assert.equal(rt.log.boots, 1);

  rt.api.teardown();
  await flush();
  assert.equal(stale.run.value.status, "terminated");

  const second = rt.gateBoot();
  const live = runOptions(rt, "c2");
  await flush();
  assert.equal(rt.log.boots, 2);
  assert.equal(rt.api.status().booting, true);

  first.resolve();
  await flush();
  assert.equal(rt.api.status().booting, true, "the live boot still owns the handle");

  second.resolve();
  await flush();
  assert.equal(rt.api.status().booted, true);
  assert.equal(rt.log.boots, 2, "no third pod");
  assert.equal(rt.log.runs.length, 1, "only the live run reached a machine");

  rt.emit(EXIT_0);
  await flush();
  assert.equal(live.run.value.status, "exited");
});

test("the reporting horizon reports without ending the run", async (t) => {
  // Settling here closed the output sink inside `run`'s `finally`, so the
  // panel said "output keeps arriving" while the sink was writing into a
  // buffer nobody would flush — and the cell went idle, taking the only
  // control that could stop the program with it.
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const rt = makeRuntime();
  const tap = runOptions(rt);
  await flush();

  rt.emit("working\n");
  await flush();
  assert.equal(tap.text, "working");

  t.mock.timers.tick(UNKNOWN_AFTER_MS + 1);
  await flush();
  assert.ok(tap.stages.includes("unknown"), "the horizon is reported");
  assert.equal(tap.run.settled, false, "the run is still running, because it is");

  rt.emit("more\n");
  await flush();
  assert.equal(tap.text, "working\nmore", "output past the horizon still arrives");

  // And the one lever there is still works.
  rt.api.teardown();
  await flush();
  assert.equal(tap.run.value.status, "terminated");
});

test("the pod outlives an editor round trip", async (t) => {
  // The editor tells authors to switch to Preview to run a Linux cell, and
  // that toggle disposes the whole cell subtree. A grace sized for a same-tick
  // remount charged 10 tokens and wiped the shared filesystem on every edit,
  // which is a month's allowance in about a hundred round trips.
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const rt = makeRuntime();
  rt.api.retain("n1", "c1");
  const tap = runOptions(rt);
  await flush();
  rt.emit(EXIT_0);
  await flush();
  assert.equal(tap.run.value.status, "exited");
  assert.equal(rt.api.status().booted, true);

  // Preview -> Edit.
  rt.api.release("c1");
  t.mock.timers.tick(60000);
  await flush();
  assert.equal(rt.api.status().booted, true, "a minute of editing costs nothing");

  // Edit -> Preview: the remount reclaims the machine and its filesystem.
  rt.api.retain("n1", "c1");
  t.mock.timers.tick(TEARDOWN_GRACE_MS * 2);
  await flush();
  assert.equal(rt.api.status().booted, true, "a mounted Linux cell holds the machine");

  // Nothing mounted for the whole window: the Workers are collected.
  rt.api.release("c1");
  t.mock.timers.tick(TEARDOWN_GRACE_MS + 1);
  await flush();
  assert.equal(rt.api.status().booted, false);
  assert.equal(rt.log.boots, 1, "one boot for the whole session");
});

test("a different notebook takes the machine with it, but not the run that asked", async () => {
  // The one teardown nobody asks for, and the reason the grace above is safe:
  // navigating to another notebook does not wait for it.
  //
  // The second half is the trap. That teardown bumps the generation, and a
  // run comparing itself against a sample taken above it reports ITSELF
  // terminated — the first Linux cell run after switching notebooks would
  // boot a machine, hand it nothing, and say the machine went away.
  const rt = makeRuntime();
  rt.api.retain("n1", "c1");
  const first = runOptions(rt, "c1");
  await flush();
  rt.emit(EXIT_0);
  await flush();
  assert.equal(first.run.value.status, "exited");

  const second = runOptions(rt, "c9", "n2");
  await flush();
  assert.equal(rt.log.boots, 2, "a second notebook is a second machine");
  assert.equal(rt.api.status().notebookId, "n2");
  assert.equal(rt.log.runs.length, 2, "and the run that asked for it still runs");
  assert.equal(rt.log.runs[1].pod, 2, "on the new machine");

  rt.emit(EXIT_0);
  await flush();
  assert.deepEqual(second.run.value, { status: "exited", exitCode: 0, inferred: false });
});
