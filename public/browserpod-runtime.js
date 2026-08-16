/**
 * IronpadPod — the BrowserPod runtime behind Linux cells (PRD-0066 T-005).
 *
 * A Linux cell is a whole Rust program compiled to
 * `wasm32-browserpod-linux-musl` and run as a real process under BrowserPod's
 * in-browser kernel. This module owns the pod: booting it, writing binaries
 * into its filesystem, running them, streaming their output back, and tearing
 * the whole thing down.
 *
 * Four rules shape everything below.
 *
 * 1. ONE POD PER NOTEBOOK, booted lazily on the first Linux-cell run and
 *    EPHEMERAL (no `storageKey`). Cells share a filesystem and a working
 *    directory, and that shared filesystem IS the piping model — there is no
 *    typed piping into or out of a Linux cell. Persistence is deliberately
 *    absent: a notebook whose behaviour depended on invisible history would
 *    give its author and its reader different results.
 *
 * 2. A BOOT COSTS A METERED TOKEN (10 of ~1,000/month, flat and
 *    duration-independent — a pod held idle for five minutes costs exactly
 *    what one held for a second does). So boots are the only currency: holding
 *    a pod for a whole session is free, and nothing may boot one without a
 *    click. This file never boots on load, never on autorun, never on a timer.
 *
 * 3. THIS FILE IS LOADED ON DEMAND, from the Rust side, at the moment a user
 *    clicks Run on a Linux cell. It is deliberately NOT in `script-loader.js`'s
 *    route table: whether a page has a Linux cell is a property of its CONTENT,
 *    not of its route, and the requirement is that a notebook with no Linux
 *    cell never touches their CDN at all. Demand-loading satisfies that for
 *    every route without enumerating any.
 *
 * 4. A CDN FAILURE IS A CELL-LEVEL ERROR, never a broken page. Every entry
 *    point below rejects with a sentence a reader can act on, and the Rust
 *    side renders it inline on the cell that asked.
 *
 * ── Three API facts that cost a pod boot each to discover ──────────────────
 *
 * `run()` resolves to a bare PID NUMBER, not a process object: its prototype
 * is `Number.prototype`, and there is no `wait`, `exitCode` or `kill`. So
 * completion must be inferred (see `SENTINEL_RE`) and Terminate has no lever
 * short of dropping the whole pod.
 *
 * `onOutput` hands back a view over RESIZABLE shared memory, which
 * `TextDecoder` refuses outright ("The provided ArrayBuffer value must not be
 * resizable"). It must be copied first. Worse, an exception thrown anywhere
 * inside that callback WEDGES `run()` so it never resolves — the failure
 * presents as a hang rather than an error, which is why the whole callback
 * body sits in a try/catch that can only ever drop output on the floor.
 *
 * `createFile(path, mode)` takes `mode: "binary"`. Their docs say only
 * `mode: string`.
 */

// Re-entrant load is harmless but must not reset the state below: the Rust
// side memoises the script tag, and a second one would otherwise drop a live
// pod on the floor and charge a fresh token for the next run.
if (!window.IronpadPod) {
  window.IronpadPod = (function () {
    "use strict";

    // The SDK's npm package is nothing but a dynamic import of this URL, so
    // importing it directly removes an indirection and keeps the version
    // pinned where it can be read.
    var SDK_URL = "https://rt.browserpod.io/3.0.1/browserpod.js";

    // Every cell runs with this as its cwd, which is what makes the shared
    // filesystem usable as the piping model: cell 1 writes `data.txt`, cell 2
    // opens `data.txt`, and neither has to name an absolute path.
    var WORKDIR = "/home/ironpad";
    // Compiled binaries live out of the way, under a dot-directory, so a cell
    // that lists its own working directory shows the notebook's files rather
    // than ironpad's plumbing.
    var BIN_DIR = WORKDIR + "/.ironpad-bin";

    // Import + boot budget. Boot measures 653-1491ms; this is not a
    // performance bound, it is the difference between an unreachable CDN
    // showing up as a notice and showing up as a cell that spins forever.
    var BOOT_TIMEOUT_MS = 30000;

    // Fallback completion inference, used only when the shell sentinel is
    // unavailable: a program is taken to have finished once it has produced
    // output and then gone quiet for this long. Wrong for a program that
    // pauses mid-run, which is exactly why it is the fallback and not the
    // mechanism.
    var QUIET_MS = 1200;

    // Nothing here kills a process, so this is not a timeout in any sense: it
    // is the point at which the UI stops claiming to know what the program is
    // doing. The run stays open, output keeps streaming, and Terminate stays
    // the only way to actually stop it.
    var UNKNOWN_AFTER_MS = 300000;

    // Output buffer caps. A runaway program can print faster than any UI can
    // render, so the buffer is bounded here rather than in the component.
    var MAX_LINES = 2000;
    var MAX_LINE_CHARS = 4000;

    // Longest half-arrived escape sequence worth holding back. A program that
    // emits an ESC and then megabytes without ever terminating the sequence
    // would otherwise buffer all of it; past this the partial is printed as
    // the ordinary text it has turned out to be.
    var MAX_PENDING_ESCAPE = 64;

    // Coalescing window for pushing text into the Leptos signal. A chatty
    // program emits hundreds of chunks a second and each one would otherwise
    // cross the wasm boundary and re-render.
    var FLUSH_MS = 80;

    // Writing a half-megabyte binary into a healthy pod is instant, so this is
    // not a performance bound either: it is the difference between a wedged
    // filesystem call showing up as a notice and showing up as a cell stuck on
    // "starting machine" forever.
    var WRITE_TIMEOUT_MS = 30000;

    // How long the pod outlives the last Linux cell that unmounted.
    //
    // Sized by the metering, not by the framework: a boot costs 10 tokens of
    // roughly a thousand a month and HOLDING one costs nothing, so the only
    // thing worth minimising is how often a pod is created. The number that
    // matters is the authoring loop — the editor tells authors to switch to
    // Preview to run a cell, and that toggle disposes the whole subtree — so a
    // grace sized for a same-tick remount (it was 250ms) charged 10 tokens
    // per edit and wiped the shared filesystem between them, exhausting a
    // month's allowance in about a hundred round trips with no readers at all.
    //
    // Minutes rather than never: a reader who navigates to a different page in
    // the SPA leaves Workers and a shared memory behind, and nothing else in
    // this file would ever collect them. Opening a different notebook still
    // tears down immediately (`claimNotebook`), and so does Terminate.
    var TEARDOWN_GRACE_MS = 300000;

    // ── Completion, inferred ────────────────────────────────────────────────
    //
    // `run()` resolves at process START, so the program's exit has to arrive
    // through the only channel there is: its own output. The program is run
    // under `/bin/sh`, which prints this marker after it, carrying the exit
    // status the SDK does not expose.
    //
    // U+001E (record separator) delimits it because it cannot appear in
    // ordinary program output by accident, and the status is matched as
    // DIGITS on purpose: if a pod build ever echoes the command line, the echo
    // contains the literal `$?` and cannot be mistaken for the expansion.
    var SENTINEL_RE = /\x1e_ironpad_exit:(\d+)\x1e/;
    // Stripped from what the reader sees, in both forms (expanded and echoed).
    var SENTINEL_STRIP_RE = /\x1e_ironpad_exit:[^\x1e]*\x1e/g;

    // `sh` reporting that it could not execute the binary at all. Paired with
    // an exit status of 126/127 this identifies a pod whose shell cannot run
    // our binaries, which is the one case that falls back to a direct
    // `pod.run` and quiet-timeout inference. Both signals are required
    // together: a user program is allowed to exit 127, and a user program is
    // allowed to print anything, but not both at once.
    var NOEXEC_RE = /^(?:\/bin\/)?sh:.*(?:Permission denied|not found|cannot execute|Exec format error)/m;

    var state = {
      apiKey: null,
      pod: null,
      // The in-flight boot, and ONLY ever the current one: a boot is never
      // cancelled (the CDN work is already in flight and the token already
      // spent), so a slow first attempt can land after a teardown has started
      // a second. Whoever clears this handle must therefore check that it is
      // still theirs, or a stale boot nulls a live one and the next click
      // boots a third pod on top of two orphans.
      //
      // A boot that lands after a teardown is DISCARDED, not adopted: the
      // machine the user dropped must not come back, and its 10 tokens are
      // gone either way.
      booting: null,
      notebookId: null,
      // Bumped by every teardown. An in-flight run compares against it and
      // goes quiet rather than reporting into a pod that no longer exists.
      generation: 0,
      boots: 0,
      running: 0,
      // Cells currently mounted. The pod is torn down when the last one goes.
      retained: {},
      retainedCount: 0,
      teardownTimer: null,
      // Content hashes already written into THIS pod's filesystem, so a rerun
      // of unchanged code skips the write entirely. There is no unlink in the
      // SDK, so identity is the only way to keep a rerun from accumulating
      // half-megabyte files.
      written: {},
      // Everything a teardown has to settle, each as the function that settles
      // it. Nothing here can kill a process, so a run whose pod was dropped
      // would otherwise sit unresolved with the cell busy and its Stop button
      // dead. Covers the WHOLE run, not just the part under `execute`: the
      // boot and the binary write are precisely the phases where the pod can
      // be pulled and there is no process yet to report anything.
      pending: [],
      // Set false permanently for a pod whose `/bin/sh` cannot exec our
      // binaries; every later run in it skips straight to the direct path.
      shellExec: true,
    };

    // ── Errors ──────────────────────────────────────────────────────────────

    /** An error whose message is meant to be read by the person who clicked. */
    function podError(message) {
      var e = new Error(message);
      e.ironpad = true;
      return e;
    }

    function describe(err) {
      if (!err) return "unknown error";
      if (err.ironpad) return err.message;
      return String((err && err.message) || err);
    }

    /**
     * What a phase awaits alongside its real work so a teardown can settle it.
     *
     * Resolving with this object rather than rejecting keeps a cancelled run
     * out of the error path: the pod going away is an outcome (`terminated`),
     * not a failure to report to the reader.
     */
    var CANCELLED = { cancelled: true };

    function makeCancelToken() {
      var fire;
      var promise = new Promise(function (resolve) {
        fire = function () {
          resolve(CANCELLED);
        };
      });
      return { promise: promise, fire: fire };
    }

    function removePending(fn) {
      var at = state.pending.indexOf(fn);
      if (at !== -1) state.pending.splice(at, 1);
    }

    function terminatedResult() {
      return { status: "terminated", exitCode: null, inferred: false };
    }

    /**
     * Reject after `ms` without cancelling `promise`.
     *
     * The pod boot behind it keeps going and stays memoised: the token is
     * already spent, so a slow boot that eventually lands should serve the
     * next click rather than be discarded.
     */
    function withTimeout(promise, ms, message) {
      return new Promise(function (resolve, reject) {
        var settled = false;
        var timer = setTimeout(function () {
          if (settled) return;
          settled = true;
          reject(podError(message));
        }, ms);
        promise.then(
          function (v) {
            if (settled) return;
            settled = true;
            clearTimeout(timer);
            resolve(v);
          },
          function (e) {
            if (settled) return;
            settled = true;
            clearTimeout(timer);
            reject(e);
          },
        );
      });
    }

    // ── Boot ────────────────────────────────────────────────────────────────

    async function bootPod() {
      // Pods need `SharedArrayBuffer` (their kernel runs a Worker per thread
      // over one shared memory), which needs cross-origin isolation. The
      // server sets COOP/COEP globally so the app itself always qualifies; a
      // cross-origin iframe does not, and says so here rather than failing
      // somewhere inside their kernel.
      if (!self.crossOriginIsolated) {
        throw podError(
          "Linux cells need a cross-origin-isolated page, which this frame is not. Open the notebook directly to run it.",
        );
      }
      if (!state.apiKey) {
        throw podError(
          "This ironpad instance has no BrowserPod key configured, so Linux cells cannot run here.",
        );
      }

      var mod;
      try {
        // Dynamic, never a static import: this is the only line in ironpad
        // that talks to rt.browserpod.io, and it must not run for a notebook
        // that has no Linux cell.
        mod = await import(SDK_URL);
      } catch (e) {
        throw podError(
          "Could not load the Linux runtime from rt.browserpod.io (" +
            describe(e) +
            "). The rest of the notebook is unaffected.",
        );
      }
      if (!mod || !mod.BrowserPod) {
        throw podError("The Linux runtime loaded but exposed no BrowserPod.");
      }

      var pod;
      try {
        // No `storageKey`: ephemeral by design (see rule 1).
        pod = await mod.BrowserPod.boot({ apiKey: state.apiKey });
      } catch (e) {
        throw podError("The Linux machine failed to boot: " + describe(e));
      }

      state.boots += 1;

      // One shared working directory for the whole notebook. `recursive`
      // because nothing guarantees `/home` exists in their image.
      try {
        await pod.createDirectory(WORKDIR, { recursive: true });
        await pod.createDirectory(BIN_DIR, { recursive: true });
      } catch (e) {
        throw podError("The Linux machine booted but has no writable home: " + describe(e));
      }

      return pod;
    }

    /**
     * Claim the notebook, dropping another notebook's machine if one is up.
     *
     * The one teardown that happens without a user asking for it, and the one
     * a run must perform BEFORE sampling the generation it will check itself
     * against: it is part of that run's setup rather than an interruption of
     * it, and a run that tore down its predecessor and then noticed the bump
     * would report itself terminated without ever starting.
     */
    function claimNotebook(notebookId) {
      if (state.notebookId !== null && state.notebookId !== notebookId) {
        teardown();
      }
      state.notebookId = notebookId;
    }

    /**
     * The notebook's pod, booting it on first use.
     */
    function ensurePod(notebookId) {
      claimNotebook(notebookId);
      cancelPendingTeardown();
      if (state.pod) return Promise.resolve(state.pod);
      if (!state.booting) {
        var generation = state.generation;
        // `attempt` is compared before either handler clears the handle: a
        // teardown mid-boot starts a fresh one, and an unconditional clear
        // would null the LIVE boot from the stale boot's own callback.
        var attempt = bootPod().then(
          function (pod) {
            if (state.booting === attempt) state.booting = null;
            if (state.generation !== generation) {
              // Torn down while booting. The token is spent either way, but
              // the machine is not kept: the user asked for it to go, and
              // nothing else will ever hold a reference to it again, so this
              // is the only chance to drop its Workers.
              disposePod(pod);
              return pod;
            }
            state.pod = pod;
            state.shellExec = true;
            state.written = {};
            return pod;
          },
          function (e) {
            if (state.booting === attempt) state.booting = null;
            throw e;
          },
        );
        state.booting = attempt;
      }
      return state.booting;
    }

    // ── Output sink ─────────────────────────────────────────────────────────

    /**
     * Turns the pod's raw pty bytes into the plain text a panel can render.
     *
     * Handles the three things a terminal does that a `<pre>` does not: ANSI
     * escapes (stripped, not interpreted — this is a log view, not an
     * emulator), a lone carriage return (rewrites the current line, which is
     * how every progress bar works), and a backspace.
     *
     * It owns the whole buffer and hands the component a complete snapshot
     * rather than deltas, so the component holds no terminal state at all and
     * the line rewriting has exactly one implementation.
     */
    function makeSink(onText) {
      var decoder = new TextDecoder("utf-8");
      var lines = [];
      var cur = "";
      // Cursor column within `cur`. A terminal's carriage return moves the
      // cursor rather than clearing the line, so `10%\r100%` and `aaa\rcc`
      // do different things: the first replaces, the second leaves `cca`.
      // Erasing instead was measured against a live pod and was wrong for
      // exactly the second case.
      var col = 0;
      var pending = "";
      var trimmed = false;
      var timer = null;
      var dirty = false;
      var closed = false;

      function render() {
        var body = lines.join("\n");
        if (cur) body = body ? body + "\n" + cur : cur;
        return trimmed ? "[earlier output trimmed]\n" + body : body;
      }

      function flush() {
        dirty = false;
        try {
          onText(render());
        } catch (e) {
          // The component went away mid-run. Its output has nowhere to go,
          // which is fine; throwing here would wedge `run()`.
          closed = true;
        }
      }

      function schedule() {
        if (closed) return;
        if (timer) {
          dirty = true;
          return;
        }
        flush();
        timer = setTimeout(function () {
          timer = null;
          if (dirty) schedule();
        }, FLUSH_MS);
      }

      function feed(text) {
        for (var i = 0; i < text.length; i++) {
          var ch = text[i];
          if (ch === "\n") {
            lines.push(cur);
            cur = "";
            col = 0;
            if (lines.length > MAX_LINES) {
              lines.splice(0, lines.length - MAX_LINES);
              trimmed = true;
            }
          } else if (ch === "\r") {
            // Back to column 0. CRLF was normalised away before this loop, so
            // this really is the progress-bar case.
            col = 0;
          } else if (ch === "\b") {
            if (col > 0) col -= 1;
          } else if (ch === "\x07") {
            // BEL. Nothing to draw and nothing to ring.
          } else if (col < MAX_LINE_CHARS) {
            cur =
              col < cur.length ? cur.slice(0, col) + ch + cur.slice(col + 1) : cur + ch;
            col += 1;
          }
        }
      }

      return {
        /** The `onOutput` callback. MUST NOT THROW: an exception here wedges `run()`. */
        write: function (buffer) {
          try {
            // The view is over resizable shared memory, which `TextDecoder`
            // rejects; `.slice()` copies it into a plain buffer first. The
            // decoder is streaming, so a multi-byte character split across
            // two chunks survives.
            var bytes = new Uint8Array(buffer).slice();
            var text = pending + decoder.decode(bytes, { stream: true });
            pending = "";

            // Complete sentinels first, so the trailing separator of a whole
            // one is never mistaken for the start of a half-arrived one.
            var exit = SENTINEL_RE.exec(text);
            text = text.replace(SENTINEL_STRIP_RE, "");

            // Then hold back whatever a chunk boundary cut in half, so the
            // next chunk completes it instead of its innards being printed.
            //
            // Four shapes are incomplete: a lone ESC, a CSI whose final byte
            // has not arrived, an unterminated OSC, and a partial sentinel.
            // The first attempt matched `ESC [^@-~]*$`, which never fired at
            // all: `[` is INSIDE `@-~`, so an `ESC [` at a boundary fell
            // through to the two-character-escape rule and the parameters
            // printed as text ("31mred"). Found by feeding the sink split
            // chunks, not by reading the pattern.
            //
            // The sentinel case matters more than the cosmetic ones: split in
            // half it would never match, and the cell would sit "running"
            // until the reporting horizon instead of finishing.
            var partial = text.match(
              /(?:\x1b$|\x1b\[[0-9;?<>=]*[ -\/]*$|\x1b\][^\x07\x1b]*$|\x1e[^\x1e]{0,32}$)/,
            );
            if (partial && partial[0].length <= MAX_PENDING_ESCAPE) {
              pending = partial[0];
              text = text.slice(0, text.length - partial[0].length);
            }

            // Then the complete escapes. This is a log view, not a terminal
            // emulator: colour and cursor addressing are dropped rather than
            // interpreted, and only the carriage return survives (in `feed`),
            // because that one carries meaning a reader would otherwise lose.
            text = text
              .replace(/\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)/g, "")
              .replace(/\x1b\[[0-9;?]*[ -\/]*[@-~]/g, "")
              .replace(/\x1b[@-_]/g, "")
              .replace(/\r\n/g, "\n");

            feed(text);
            schedule();
            if (exit) this.onExit(Number(exit[1]));
          } catch (e) {
            // Deliberately swallowed. See the file header: this callback is
            // the one place where an exception costs a hang.
          }
        },
        /** Replaced by the run once it is listening for the sentinel. */
        onExit: function () {},
        text: render,
        matchedNoExec: function () {
          return NOEXEC_RE.test(render());
        },
        reset: function () {
          lines = [];
          cur = "";
          col = 0;
          pending = "";
          trimmed = false;
          schedule();
        },
        close: function () {
          // Whatever a chunk boundary cut in half is ordinary text now, since
          // nothing further is coming to complete it. Held back and never fed,
          // it was simply lost: `a\x1eb\n` followed by the sentinel rendered
          // `a` and dropped the rest, because the sentinel scanner holds back
          // everything from a record separator onwards.
          //
          // The first character says which of the four incomplete shapes it
          // is, and that is the whole decision: a partial escape sequence is
          // control bytes a log view drops anyway, while a partial sentinel is
          // one separator in front of real program output.
          var tail = pending.charAt(0) === "\x1e" ? pending.slice(1) : "";
          pending = "";
          if (tail) feed(tail);
          closed = false;
          if (timer) {
            clearTimeout(timer);
            timer = null;
          }
          flush();
          closed = true;
        },
      };
    }

    // ── Writing the binary ──────────────────────────────────────────────────

    /** FNV-1a over the binary, only ever compared against itself. */
    function contentHash(bytes) {
      var h = 0x811c9dc5;
      for (var i = 0; i < bytes.length; i++) {
        h ^= bytes[i];
        h = (h + ((h << 1) + (h << 4) + (h << 7) + (h << 8) + (h << 24))) >>> 0;
      }
      return h.toString(16) + "-" + bytes.length.toString(16);
    }

    /** Filesystem-safe stem for a cell id (which is a uuid, but not by contract). */
    function safeName(cellId) {
      return String(cellId).replace(/[^A-Za-z0-9_-]/g, "_").slice(0, 64);
    }

    /**
     * Put `bytes` in the pod at a content-addressed path and return it.
     *
     * Content-addressed because the SDK exposes no unlink: rerunning an edited
     * cell twenty times would otherwise leave twenty half-megabyte files, and
     * reusing one path would leave the tail of a longer previous binary behind
     * a shorter new one.
     */
    async function writeBinary(pod, cellId, bytes) {
      var path = BIN_DIR + "/" + safeName(cellId) + "-" + contentHash(bytes);
      if (state.written[path]) return path;
      var file;
      try {
        file = await pod.createFile(path, "binary");
      } catch (e) {
        throw podError("Could not write the program into the Linux machine: " + describe(e));
      }
      try {
        // Hand over a buffer that is EXACTLY the program. A `Uint8Array` that
        // is a view rather than an owner would otherwise pass its whole
        // backing store, which on the wasm side is the entire linear memory.
        var payload =
          bytes.byteOffset === 0 && bytes.byteLength === bytes.buffer.byteLength
            ? bytes.buffer
            : bytes.slice().buffer;
        var wrote = await file.write(payload);
        if (typeof wrote === "number" && wrote < bytes.byteLength) {
          throw podError(
            "Only " + wrote + " of " + bytes.byteLength + " bytes reached the Linux machine.",
          );
        }
      } finally {
        // `close` appears in their type definitions but not always on the
        // returned object's prototype.
        if (file && typeof file.close === "function") {
          try {
            await file.close();
          } catch (e) {
            /* nothing actionable */
          }
        }
      }
      state.written[path] = true;
      return path;
    }

    // ── Running ─────────────────────────────────────────────────────────────

    function shellQuote(path) {
      return "'" + String(path).replace(/'/g, "'\\''") + "'";
    }

    /**
     * The command `/bin/sh` runs: the program, then the exit sentinel.
     *
     * `chmod` is folded in because nothing in the SDK sets a mode on a created
     * file and `sh` will refuse to exec without one; its failure is ignored,
     * since the direct-exec fallback covers the case where it mattered.
     *
     * It runs on EVERY run, and that is not redundancy to tidy away later.
     * The bytes arrive from a content-addressed cache — the browser's
     * IndexedDB blob store or an immutable share snapshot — and nothing in
     * either carries a file mode, so the mode can only ever come from here.
     * The write itself is skipped for a rerun of unchanged code (see
     * `writeBinary`); the chmod deliberately is not.
     */
    function sentinelCommand(binPath) {
      var q = shellQuote(binPath);
      return (
        "chmod +x " +
        q +
        " 2>/dev/null; " +
        q +
        "; __ip=$?; printf '\\036_ironpad_exit:%d\\036' \"$__ip\""
      );
    }

    /**
     * Run one Linux cell.
     *
     * @param {object} opts
     * @param {string} opts.notebookId  Which pod this belongs to.
     * @param {string} opts.cellId      Names the binary in the pod.
     * @param {Uint8Array} opts.bytes   The compiled program.
     * @param {(text: string) => void} opts.onText   Full output snapshot, throttled.
     * @param {(stage: string) => void} [opts.onStage]  "booting", "running", then
     *   possibly "unknown" — see {@link UNKNOWN_AFTER_MS}.
     * @returns {Promise<{status: string, exitCode: number|null, inferred: boolean}>}
     *
     * `status` is one of `exited` (the sentinel reported, `exitCode` is real),
     * `finished` (inferred from a quiet terminal, `exitCode` null), or
     * `terminated` (the pod went away under it).
     *
     * Passing the reporting horizon is deliberately NOT one of them: the run
     * is still running, so it stays open and keeps streaming, and the horizon
     * arrives through `onStage` instead.
     */
    async function run(opts) {
      var notebookId = opts.notebookId;
      var stage = opts.onStage || function () {};
      var sink = makeSink(opts.onText);

      // Switching notebooks drops the previous machine, and that is this
      // run's own doing — so it happens first, and the generation below is
      // sampled after it.
      claimNotebook(notebookId);

      // Sampled BEFORE the boot, not after it. A teardown during the boot
      // bumps the generation first, so a sample taken afterwards compares
      // equal to it for the rest of the run: every later check passed, and the
      // program went on to run on a pod `state.pod` no longer pointed at,
      // unreachable by any future teardown.
      var generation = state.generation;

      // The boot and the write are the two phases with no process to report
      // anything, so a teardown in either had nothing to settle and left this
      // promise pending for good — the cell wedged on "starting machine" with
      // a dead Stop button, recoverable only by reloading the page. This is
      // what teardown settles for them.
      var token = makeCancelToken();
      state.pending.push(token.fire);

      try {
        stage("booting");
        var pod = await Promise.race([
          token.promise,
          withTimeout(
            ensurePod(notebookId),
            BOOT_TIMEOUT_MS,
            "The Linux machine did not start within " +
              Math.round(BOOT_TIMEOUT_MS / 1000) +
              "s. rt.browserpod.io may be unreachable.",
          ),
        ]);
        if (pod === CANCELLED || state.generation !== generation) return terminatedResult();

        var binPath = await Promise.race([
          token.promise,
          withTimeout(
            writeBinary(pod, opts.cellId, opts.bytes),
            WRITE_TIMEOUT_MS,
            "The program did not reach the Linux machine within " +
              Math.round(WRITE_TIMEOUT_MS / 1000) +
              "s.",
          ),
        ]);
        if (binPath === CANCELLED || state.generation !== generation) return terminatedResult();

        stage("running");
        state.running += 1;
        try {
          var result = await execute(pod, sink, binPath, generation, state.shellExec, stage);
          // A pod whose shell cannot exec our binaries says so once and is
          // believed for the rest of its life; the retry re-runs the program,
          // which is a side effect worth paying exactly once.
          if (result.noExec) {
            state.shellExec = false;
            sink.reset();
            result = await execute(pod, sink, binPath, generation, false, stage);
          }
          return { status: result.status, exitCode: result.exitCode, inferred: result.inferred };
        } finally {
          state.running -= 1;
        }
      } finally {
        removePending(token.fire);
        sink.close();
      }
    }

    /**
     * One attempt, either under the shell (sentinel completion, real exit
     * code) or directly (quiet-timeout inference, no exit code).
     */
    function execute(pod, sink, binPath, generation, useShell, stage) {
      return new Promise(function (resolve, reject) {
        var settled = false;
        var quietTimer = null;
        var capTimer = null;
        var sawOutput = false;

        function finish(status, exitCode, inferred, noExec) {
          if (settled) return;
          settled = true;
          removePending(finish);
          if (quietTimer) clearTimeout(quietTimer);
          if (capTimer) clearTimeout(capTimer);
          resolve({
            status: status,
            exitCode: typeof exitCode === "number" ? exitCode : null,
            inferred: !!inferred,
            noExec: !!noExec,
          });
        }

        state.pending.push(finish);

        sink.onExit = function (code) {
          if (state.generation !== generation) return finish("terminated", null, false, false);
          // 126/127 alone is a legal exit status for a user program, so the
          // shell's own diagnostic has to be there too before this counts as
          // "this pod cannot exec our binaries".
          var noExec = (code === 126 || code === 127) && sink.matchedNoExec();
          finish("exited", code, false, noExec);
        };

        function bumpQuiet() {
          if (!useShell) {
            sawOutput = true;
            if (quietTimer) clearTimeout(quietTimer);
            quietTimer = setTimeout(function () {
              // No sentinel available, so "it stopped talking" is the only
              // evidence there is. Deliberately weak, and reported as such.
              finish("finished", null, true, false);
            }, QUIET_MS);
          }
        }

        var userOnText = sink.write;
        var wrapped = {
          onOutput: function (buffer) {
            // Guarded inside `write` already; this wrapper only adds the
            // quiet-timer bump, and must not throw either.
            try {
              userOnText.call(sink, buffer);
              bumpQuiet();
            } catch (e) {
              /* see the file header */
            }
          },
        };

        capTimer = setTimeout(function () {
          // Nothing here can kill a process, so this REPORTS rather than
          // settles. Settling was a promise the code could not keep: `run`'s
          // `finally` closes the sink the moment this resolves, so the panel
          // said "output keeps arriving" while the sink was already writing
          // into a buffer nobody would ever flush, and the cell went idle so
          // the only control that could stop the program disappeared with it.
          //
          // Leaving the run open keeps all three true at once: the sink lives,
          // the caller's future lives (and with it the callbacks output flows
          // through), and Terminate stays the one lever there is.
          try {
            stage("unknown");
          } catch (e) {
            /* the component went away; the run is unaffected */
          }
        }, UNKNOWN_AFTER_MS);

        pod
          .createCustomTerminal({ onOutput: wrapped.onOutput })
          .then(function (terminal) {
            if (state.generation !== generation) {
              return finish("terminated", null, false, false);
            }
            var exe = useShell ? "/bin/sh" : binPath;
            var args = useShell ? ["-c", sentinelCommand(binPath)] : [];
            return pod
              .run(exe, args, { terminal: terminal, cwd: WORKDIR, echo: false })
              .then(function () {
                // Resolves with a PID at process START. Completion arrives
                // through the sentinel or the quiet timer, never here.
                if (!useShell && !sawOutput) bumpQuiet();
              });
          })
          .catch(function (e) {
            if (settled) return;
            if (useShell) {
              // No `/bin/sh` in this image at all: fall back to direct exec
              // the same way an unexecutable binary does.
              return finish("exited", 127, false, true);
            }
            settled = true;
            removePending(finish);
            if (quietTimer) clearTimeout(quietTimer);
            if (capTimer) clearTimeout(capTimer);
            reject(podError("The program could not be started: " + describe(e)));
          });
      });
    }

    // ── Lifetime ────────────────────────────────────────────────────────────

    function cancelPendingTeardown() {
      if (state.teardownTimer) {
        clearTimeout(state.teardownTimer);
        state.teardownTimer = null;
      }
    }

    /**
     * Best-effort disposal of a pod nothing will ever reference again.
     *
     * The SDK's type definitions expose no disposal method. This probe costs
     * nothing and starts working the day they add one; until then a dropped
     * pod's Workers live until the page unloads, and the generation bump is
     * what stops it from reporting into live UI in the meantime.
     */
    function disposePod(pod) {
      if (!pod) return;
      var names = ["destroy", "dispose", "shutdown", "close", "exit", "kill"];
      for (var i = 0; i < names.length; i++) {
        if (typeof pod[names[i]] === "function") {
          try {
            pod[names[i]]();
          } catch (e) {
            /* best effort */
          }
          return;
        }
      }
    }

    /**
     * Drop the pod.
     *
     * `run()` gives back a PID with no `kill`, so this is the only lever there
     * is over a running program — which is exactly why Terminate is documented
     * as restarting the machine rather than stopping the cell.
     */
    function teardown() {
      cancelPendingTeardown();
      var pod = state.pod;
      state.generation += 1;
      // Settle every in-flight run before the pod goes, so a cell whose
      // machine was pulled reports "terminated" now rather than hanging.
      var inflight = state.pending.slice();
      state.pending.length = 0;
      for (var p = 0; p < inflight.length; p++) {
        try {
          inflight[p]("terminated", null, false, false);
        } catch (e) {
          /* a settled finisher is a no-op */
        }
      }
      state.pod = null;
      state.booting = null;
      state.notebookId = null;
      state.written = {};
      state.shellExec = true;
      disposePod(pod);
    }

    return {
      /** Whether a key has been handed over yet (the Rust side fetches it once). */
      hasKey: function () {
        return !!state.apiKey;
      },
      configure: function (apiKey) {
        state.apiKey = apiKey || null;
      },
      /** Whether this page can host a pod at all. */
      isolated: function () {
        return !!self.crossOriginIsolated;
      },
      /** A Linux cell mounted. Cheap, and never boots anything. */
      retain: function (notebookId, cellId) {
        cancelPendingTeardown();
        if (!state.retained[cellId]) {
          state.retained[cellId] = true;
          state.retainedCount += 1;
        }
      },
      /**
       * A Linux cell unmounted. When the last one goes the pod goes with it,
       * after {@link TEARDOWN_GRACE_MS} — long enough that the editor's
       * Preview↔Edit round trip, which disposes and rebuilds the whole cell
       * list, keeps the machine and its filesystem instead of paying 10
       * tokens to boot an identical one.
       */
      release: function (cellId) {
        if (state.retained[cellId]) {
          delete state.retained[cellId];
          state.retainedCount -= 1;
        }
        if (state.retainedCount > 0) return;
        cancelPendingTeardown();
        state.teardownTimer = setTimeout(function () {
          state.teardownTimer = null;
          if (state.retainedCount === 0) teardown();
        }, TEARDOWN_GRACE_MS);
      },
      teardown: teardown,
      run: run,
      /** Diagnostics, and what an e2e asserts against. Boots nothing. */
      status: function () {
        return {
          booted: !!state.pod,
          booting: !!state.booting,
          boots: state.boots,
          running: state.running,
          retained: state.retainedCount,
          notebookId: state.notebookId,
        };
      },
    };
  })();
}
