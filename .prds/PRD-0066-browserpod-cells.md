---
id: PRD-0066
title: "Linux cells: a cell that is a real Linux process"
status: active
owner: "Aaron Roney"
created: 2026-08-15
updated: 2026-08-16

principles:
- "Explicit, never detected. Every other special case keeps the execution model; this one replaces it, so it is opted into, not inferred from a substring."
- "A second execution model, not a fourth build flag. Measured: 431 syscall imports against the ~12 host functions our executor provides."
- "The pod is the piping model. One pod per notebook, shared filesystem, Unix semantics. There is no typed piping here and there should not be."
- "Never autorun. Every pod boot spends a metered allowance on the owner's key; a boot must trace back to a click."
- "The notebook format names the semantics (Linux), never the vendor (BrowserPod). WALI may make the vendor swappable; the persisted document should not have to care."

references:
- name: "BrowserPod 3.0: Rust in the browser"
  url: https://labs.leaningtech.com/blog/browserpod-rust
- name: "browserpod-meta: 'free to use only for personal and open-source projects'"
  url: https://github.com/leaningtech/browserpod-meta
- name: "BrowserPod licensing (free tier requires visible link + logo)"
  url: https://browserpod.io/docs/more/licensing
- name: "WALI: Empowering WebAssembly with Thin Kernel Interfaces (EuroSys '25). MIT, WAMR-based, native-only today."
  url: https://github.com/Wasm-Thin-Kernel-Interfaces/WALI

acceptance_tests:
- id: uat-001
  name: "A Linux cell compiles server-side to a binary exporting _start"
  command: cargo make test-integration
  uat_status: unverified
- id: uat-002
  name: "A Linux cell runs in a pod and streams stdout into its panel"
  command: cargo make playwright -- linux-cells
  uat_status: unverified
- id: uat-003
  name: "Two Linux cells in one notebook share a filesystem: cell 1 writes, cell 2 reads"
  command: cargo make playwright -- linux-cells
  uat_status: unverified
- id: uat-004
  name: "A notebook with no Linux cell requests nothing from rt.browserpod.io and boots no pod"
  command: cargo make playwright -- linux-cells
  uat_status: unverified
- id: uat-005
  name: "Nothing autoruns a Linux cell: not public autorun, not Run All on load, not the agent protocol"
  command: cargo make playwright -- linux-cells
  uat_status: unverified
- id: uat-006
  name: "Attribution renders in every Linux cell frame and in no other cell"
  command: cargo make playwright -- linux-cells
  uat_status: unverified
- id: uat-007
  name: "A CDN failure degrades to an inline notice, not a broken page or a hung notebook"
  command: cargo make playwright -- linux-cells
  uat_status: unverified
- id: uat-008
  name: "Embeds refuse Linux cells via the existing cross-origin-isolation notice"
  command: cargo make playwright -- embed
  uat_status: unverified
- id: uat-009
  name: "Full gate"
  command: cargo make uat
  uat_status: unverified

tasks:
- id: T-001
  title: "Runtime spike: boot a pod in a page, run the spike binary, capture output"
  priority: 1
  status: done
  notes: "FIRST, before any model change. Answers empirically what the SDK does not document: whether Process exposes completion/exit/kill, what boot costs in wall-clock, and what a token buys. Everything below is cheaper to design once this is known."
- id: T-002
  title: "CellType::Linux in the notebook format"
  priority: 1
  status: todo
  notes: "New variant, NOT a boolean flag beside cell_type: Code. An unknown cell type must deserialize to an inert 'unsupported cell' that renders source read-only and refuses to run. The PRD-0047 lesson: an old client that silently treats it as Code would compile it to the wrong target and fail confusingly."
- id: T-003
  title: "Compile path: fourth toolchain pin, variable target triple"
  priority: 1
  status: todo
  notes: "BROWSERPOD_TOOLCHAIN = browserpod-3.0.0 (pins nightly-2026-05-19). cell_toolchain already switches by name. cache_key's target triple is currently FIXED and must become variable; its own comment anticipates this. Linux cells get shared.rs as `mod shared` but NOT ironpad-cell."
- id: T-004
  title: "Scaffold: whole programs, not fragments"
  priority: 1
  status: todo
  notes: "The author writes `fn main()`. ironpad supplies only Cargo.toml and the shared module. No cell_main wrapper, no cellN bindings, no trampoline."
- id: T-005
  title: "Pod runtime: one per notebook, lazy, ephemeral"
  priority: 1
  status: todo
  notes: "Boot on first Linux-cell run, no storageKey (ephemeral by design), teardown on navigation. Separate from executor-core.js entirely. Script loaded per-route like KaTeX/Prism so a notebook without Linux cells never touches their CDN."
- id: T-006
  title: "Streaming terminal panel"
  priority: 2
  status: todo
  notes: "createCustomTerminal({onOutput}) appends live. Existing panels are static and PanelMode::Snapshot assumes finality, so this is a new panel kind. Completion may have to be inferred (quiet timeout or sentinel) if Process exposes nothing; T-001 decides."
- id: T-007
  title: "Failure and teardown UX"
  priority: 2
  status: todo
  notes: "panic = abort, so a panic kills the process and may kill the pod. Render stderr as a cell error. If the pod died, mark every other Linux cell in the notebook stale with 'the shared Linux machine restarted'. Terminate tears down the pod, visibly."
- id: T-008
  title: "Never autorun, anywhere"
  priority: 1
  status: todo
  notes: "Public-notebook autorun skips Linux cells. Run All includes them in notebook order but only on explicit invocation. Agent `cells run` refuses them with a clear error rather than dispatching (a CLI boot has no human watching)."
- id: T-009
  title: "Attribution in the cell frame header"
  priority: 1
  status: todo
  notes: "Visible link + logo on every Linux cell, free-tier requirement. Renders nowhere else."
- id: T-010
  title: "ADD_LINUX editor button + no saved_output"
  priority: 2
  status: todo
  notes: "Third add-cell button beside ADD_CODE/ADD_MARKDOWN. Linux cells never capture saved_output; view-only pages show a Run affordance instead. capture-outputs must skip them, so the key never needs to exist in CI."
- id: T-011
  title: "PROTOCOL_VERSION 7 + live check + blob snapshots"
  priority: 2
  status: todo
  notes: "Agents read/write Linux cells like any other. check_cell runs with the target swapped (server-side, costs no tokens). Share-time blob snapshots apply unchanged once the cache key carries the triple."
- id: T-012
  title: "Docker: vendor the toolchain tarball"
  priority: 3
  status: todo
  notes: "38MB checksum-pinned tarball plus a nightly. Vendor it rather than curl|bash at image build, so the prod BUILD never depends on rt.browserpod.io being up. The RUNTIME unavoidably does."
- id: T-014
  title: "Keep pod-booting specs out of the default gate"
  priority: 1
  status: todo
  notes: "At 10 tokens/boot the e2e suite outspends visitors by an order of magnitude. Specs that need a real pod live in an opt-in `cargo make test-linux-cells`, the way test-integration is already separated for being slow; `cargo make uat` and CI must never boot one. The absence-asserting specs (uat-001 compile, uat-004 no-CDN-contact, uat-005 never-autorun, uat-008 embed refusal) stay in the normal gate since they boot nothing. uat-007 (CDN failure) is tested by BLOCKING the rt.browserpod.io hostname in Playwright rather than by booting and killing, which is cheaper and deterministic."
- id: T-013
  title: "One public notebook: a subprocess pipeline and real threads"
  priority: 3
  status: todo
  notes: "Last. Threads are the stronger demo than first credited: sched_getaffinity is imported, so available_parallelism() should return something real and a cell can fan out across actual Workers."
---

# Summary

`CellType::Linux`: an explicitly opted-in cell that compiles to
`wasm32-browserpod-linux-musl` and runs as a real Linux process under
BrowserPod's in-browser kernel. It gets a filesystem, subprocesses, sockets and
threads. Regular cells are untouched.

One pod per notebook, so Linux cells share a machine and pipe through the
filesystem the way Unix programs do.

# Problem

ironpad cells target `wasm32-unknown-unknown` in a Web Worker: no filesystem, no
process, no socket, no real thread. Correct for what cells are, and a hard
ceiling on what a notebook can demonstrate.

# Technical Approach

## What the spike established (2026-08-15)

A program using `std::fs`, `std::thread`, `std::process::Command`, `std::env`
and `std::time` compiled **first try, no source changes, in 0.99s**, to a 544KB
binary exporting `_start`, `main`, `__wasm_call_ctors` and `memory`.

The toolchain installs as an ordinary rustup toolchain
(`~/.rustup/toolchains/browserpod-3.0.0`, pinning `nightly-2026-05-19`), which
is the same mechanism `cell_toolchain` already uses for three pins.

**The import table decides the architecture:**

| module | imports |
| --- | --- |
| `i` | **431** (`__syscall_clone3`, `__syscall_futex_6`, `__syscall_execve`, …) |
| `wasi_snapshot_preview1` | 4 (args and environ only) |

Against roughly a dozen host functions our executor provides under `env`. This
cannot be a build flag on the existing path.

## How threads actually work

`std::thread::spawn` lowers to real `__syscall_clone`/`clone3` with `futex`
parking, `set_tid_address`, `gettid` and the whole `sched_*` family; `fork`,
`vfork`, `execve`, `wait4` and `pipe2` are all imported, so `Command::spawn` is
a genuine fork/exec.

Their kernel services these by spawning **a Web Worker per thread or process**,
all sharing one `WebAssembly.Memory`. The target spec is what enables it:
`--shared-memory`, `--import-memory=i,memory`, `+atomics` (real futex needs
`memory.atomic.wait/notify`), and `--export=__startThread` as the entry point
their kernel calls in the new Worker. `singlethread: true` and
`has-thread-local: false` are not contradictions: they disable *LLVM's* TLS
lowering so `__tls_base` is managed per-Worker by the kernel instead.

This is the same primitive ironpad already uses for rayon cells, taken further,
which is why Linux cells inherit the cross-origin-isolation requirement and the
no-embeds rule.

## Infrastructure already in place

- **Cross-origin isolation**: the server already sets `COOP: same-origin` and
  `COEP: require-corp` globally, which is why rayon cells work. No change.
- **CSP**: the policy is `object-src 'none'; base-uri 'self'; form-action
  'self'` and deliberately carries no `script-src`, so their dynamic import is
  not blocked. No change. (A first pass reported otherwise by matching a
  *comment* that mentions `script-src`; the policy itself does not contain it.)
- **Their CDN cooperates with COEP**: `rt.browserpod.io` returns
  `access-control-allow-origin: *` and `cross-origin-resource-policy:
  cross-origin`, verified directly. They need `SharedArrayBuffer` themselves, so
  this is designed for.

## Shape

```
regular cell  source -> CELL_TOOLCHAIN      -> wasm32-unknown-unknown -> executor -> cell_main -> typed panels
Linux cell    source -> BROWSERPOD_TOOLCHAIN-> wasm32-browserpod-...  -> pod      -> _start    -> streamed stdout
```

Two pipelines sharing the compile cache, admission control and blob snapshots,
and sharing nothing at runtime.

# Assumptions

- ironpad qualifies for free use: public, MIT, and their README says BrowserPod
  is "free to use only for personal and open-source projects".
- The owner's allowance is **10,000 tokens/month**. The unit is undefined in
  public docs (the pricing policy linked from their own README 404s), so the
  budget cannot be modelled yet. Never-autorun (T-008) exists so consumption
  stays proportional to deliberate clicks regardless of what a token turns out
  to be.

# Constraints

- **No detection.** Flipping a cell to a different runtime because someone typed
  `std::fs` would silently change its semantics and its piping.
- **No autorun.** The only unbounded-consumption path, closed by construction.
- **CDN dependency is accepted** with two hard requirements, both tested: a
  notebook with no Linux cell never touches their CDN, and a CDN failure
  degrades to an inline notice rather than a broken page.
- **Ephemeral pods.** No `storageKey`. Persistence would make a notebook's
  behaviour depend on invisible history, so the same notebook would give
  different results to its author and its reader.
- Free-tier attribution is a requirement, not a courtesy.

## What the runtime spike established (T-001, 2026-08-16)

End to end, on a COOP/COEP-isolated origin matching production: the SDK loaded,
a pod booted, our 553KB binary was written into its filesystem and executed, and
its stdout came back. Real `/proc`, real `clone3` threads, and `/bin/ls`
genuinely forked and exec'd (BusyBox) to list a file the Rust code had written.

| step | cold | warm |
| --- | --- | --- |
| SDK import | 425ms | 6ms |
| `BrowserPod.boot` | 1491ms | 653-880ms |
| `run()` returns | 615ms | 615ms |

So a first Linux-cell click costs roughly 1.5-2s of setup, amortized across the
notebook because there is one pod (T-005).

**`Process` is not an object, it is a PID.** `run()` resolves to a bare number:
its prototype is `Number.prototype` (`toExponential`, `toFixed`, …). There is no
`wait`, `exitCode`, `kill` or `then`. Consequences, both already chosen
correctly but now forced rather than preferred:

- **Completion must be inferred** (T-006). Quiet-output timeout or a sentinel is
  the primary mechanism, not a fallback.
- **Terminate must tear down the pod** (T-007). With no `kill`, there is no
  other lever.

Note the asymmetry: the *program* sees exit codes fine (`Command::output()`
reported `exit status: 0`). The gap is only between JS and the top-level
process.

**Two API traps, both of which would bite the implementation identically:**

1. `onOutput` hands a view over **resizable** (shared) memory, and `TextDecoder`
   throws `The provided ArrayBuffer value must not be resizable`. Copy first
   (`new Uint8Array(buf).slice()`). Worse: an exception thrown inside the
   callback **wedges `run()` so it never resolves**, so the failure mode is a
   hang rather than an error.
2. `createFile` mode is **`"binary"`**, not `"w"`; the docs say only
   `mode: string`. The returned object exposes just `read`/`write` on its
   prototype despite `close()` appearing in their type definitions.

## Metering, measured (2026-08-16)

**10 tokens per boot, flat, duration-independent.** Three short-lived boots cost
30 (10,000 -> 9,970). A fourth pod held **idle for 322 seconds** cost exactly 10
more (-> 9,960), 50x the lifetime at the same price.

The mechanism explains it: booting pulls five assets from their CDN
(`browserpod.js`, `kernel.wasm`, `cache.wasm`, `worker.js`, `opfs_worker.js`)
inside the first 337ms, and then makes **zero further requests to their origin**.
The runtime is entirely client-side afterwards, so there is no beacon that could
report duration. Caveat: this observes their client, not their billing; the
counter reading is the real evidence and the network trace only explains it.

So the allowance is **~1,000 pod boots/month**, about 33/day, and three
consequences follow:

- **Holding a pod for a whole notebook session is free.** T-005 needs no idle
  teardown.
- **Long-running cells cost nothing extra**, so no runtime timeout is needed for
  cost reasons (T-007's "no auto-timeout in v1" stands on its own merits).
- **Boots are the only currency.** The only thing that matters is how often a pod
  is created: one per notebook page-session, never automatic.

### The test suite is the dominant consumer, not visitors

At 10/boot, a pod-dependent spec run costs ~40 tokens; ten gate runs a day is
400/day and the monthly allowance is gone in under four weeks of ordinary
development, with no users involved. Holding pods cannot amortize this because
the cost is entirely in the create. Hence T-014.

# Open Questions

1. **Does per-cell attribution satisfy "your application must display"?**

# Non-Goals (MVP)

- Compiling Rust in the browser. Their post says planned, not implemented. If it
  lands, only the compile third of this design moves; the runtime half stands.
- Targeting WALI instead. It is MIT and open, but its only implementation is in
  WAMR, native-only, with no browser host and no Rust target. It is the reason
  the format says `Linux` rather than `BrowserPod`, not an option today.
- Typed piping into or out of Linux cells; the shared filesystem is the model.
- Running Linux cells in embeds.
- Persistent pod filesystems, notebook-level filesystem seeding, `userImage`.
- A runtime abstraction over the vendor. One implementation behind an interface
  is speculative generality; the format already carries the protection.

# History

- 2026-08-15: Created after a spike compiled a beefy program to the target on
  the first attempt.
- 2026-08-16: Rewritten after a full design grilling. Twenty decisions settled;
  see Principles and Constraints. Two earlier claims corrected: "you would build
  it twice" overstated the impact of browser-side rustc (only the compile third
  moves), and the failure UX cannot assume an exit code the SDK does not
  document. Proceeding without the licensing answers, by owner decision.
