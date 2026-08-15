---
id: PRD-0066
title: "BrowserPod cells: a cell that thinks it is Linux"
status: draft
owner: "Aaron Roney"
created: 2026-08-15
updated: 2026-08-15

principles:
- "Explicit, never detected. Every other special case keeps the execution model; this one replaces it, so it must be opted into, not inferred from a substring."
- "A second execution model, not a fourth build flag. Measured: 431 syscall imports against the ~12 host functions our executor provides."
- "Unix semantics, not typed piping. A process has stdin and stdout; it does not have cellN slots."
- "The build half is ours and cheap. The run half is rented, metered, CDN-only, and needs a key."
- "Ship nothing to /public until the token limits are known. They are not published today; the pricing policy that documents them 404s."

references:
- name: "BrowserPod 3.0: Rust in the browser"
  url: https://labs.leaningtech.com/blog/browserpod-rust
- name: "BrowserPod licensing (free for personal and open-source; attribution required on the free tier)"
  url: https://browserpod.io/docs/more/licensing
- name: "browserpod-meta README: 'proprietary software and it's free to use only for personal and open-source projects'"
  url: https://github.com/leaningtech/browserpod-meta
- name: "API key docs: BrowserPod.boot({ apiKey })"
  url: https://browserpod.io/docs/understanding-browserpod/api-key

acceptance_tests:
- id: uat-001
  name: "A BrowserPod cell compiles server-side and produces a wasm binary exporting _start"
  command: cargo make test-integration
  uat_status: unverified
- id: uat-002
  name: "A BrowserPod cell runs in the browser and its stdout becomes the cell output"
  command: cargo make playwright -- browserpod
  uat_status: unverified
- id: uat-003
  name: "BrowserPod cells never appear in typed piping: cell_deps ignores them and the cascade skips them"
  command: cargo make test
  uat_status: unverified
- id: uat-004
  name: "Attribution (link + logo) renders on any notebook containing a BrowserPod cell, and on no other"
  command: cargo make playwright -- browserpod
  uat_status: unverified
- id: uat-005
  name: "Embeds refuse BrowserPod cells with the existing cross-origin-isolation notice, not a broken pod"
  command: cargo make playwright -- embed
  uat_status: unverified
- id: uat-006
  name: "A notebook with no BrowserPod cell loads no BrowserPod script and boots no pod"
  command: cargo make playwright -- first-paint
  uat_status: unverified

tasks:
- id: T-000
  title: "BLOCKER: obtain an API key and settle licensing questions with Leaning Technologies"
  priority: 0
  status: blocked
  notes: "Owner action, not an agent action. Needs: an account at console.browserpod.io, the free-tier token limits (unpublished; the pricing policy linked from their own README 404s), confirmation that per-notebook attribution satisfies 'your application must display', and a domain lock to ironpad.twitchax.com. Nothing below ships without this."
- id: T-001
  title: "Add CellKind::BrowserPod to the notebook format"
  priority: 1
  status: todo
  notes: "Explicit opt-in, mirroring `shared: true`. NOT substring detection. Old clients must degrade safely: an unknown kind should render as inert source, never as a normal Code cell (see the PRD-0047 browser-cache lesson, where an old client silently compiled shared cells as normal ones)."
- id: T-002
  title: "Compile path: fourth toolchain pin + target"
  priority: 1
  status: todo
  notes: "BROWSERPOD_TOOLCHAIN = browserpod-3.0.0 (pins nightly-2026-05-19 underneath), --target wasm32-browserpod-linux-musl. cell_toolchain already switches by name. cache_key's target triple is currently FIXED and must become variable; the comment there already anticipates this."
- id: T-003
  title: "Runtime path: boot a Pod, run the binary, capture stdout"
  priority: 1
  status: todo
  notes: "Separate from executor-core.js entirely. `BrowserPod.boot({apiKey})`, write the binary into the pod FS via createFile, `run(exe, args, {terminal})` with createCustomTerminal's onOutput for capture. Lazy-loaded per notebook, like KaTeX/Prism."
- id: T-004
  title: "Unix semantics: stdin from upstream display output, stdout as the cell output"
  priority: 2
  status: todo
  notes: "No typed piping. cell_deps must not see BrowserPod cells as cellN producers or consumers, and the cascade must skip them."
- id: T-005
  title: "Conditional attribution component"
  priority: 1
  status: todo
  notes: "Renders link + logo when the notebook contains a BrowserPod cell. Free-tier requirement. See Open Questions: whether per-notebook satisfies 'your application must display' is unconfirmed."
- id: T-006
  title: "Embed refusal + cross-origin isolation"
  priority: 2
  status: todo
  notes: "The target sets --shared-memory and +atomics, so it needs COOP/COEP exactly as rayon cells do. Reuse the existing threaded-cell embed notice rather than inventing a second one."
- id: T-007
  title: "Error UX for panic = abort"
  priority: 2
  status: todo
  notes: "The target is panic-strategy: abort. Our panic hook cannot report a cell error the usual way; a panic takes the pod down. Decide what the frame shows."
- id: T-008
  title: "Docker image: install the toolchain in the build stage"
  priority: 2
  status: todo
  notes: "38MB checksum-pinned tarball plus a full nightly. Vendor the tarball rather than curl|bash at image build time, so a prod image never depends on rt.browserpod.io being up (the RUNTIME already does; the BUILD need not)."
- id: T-009
  title: "One public notebook demonstrating it"
  priority: 3
  status: todo
  notes: "Depends on T-000 and on the capture question in Open Questions. Deliberately last."
---

# Summary

A new, explicitly opted-in cell kind that compiles to
`wasm32-browserpod-linux-musl` and runs under BrowserPod's in-browser Linux
kernel, so a cell can use `std::fs`, `std::process`, `std::net` and threads.
Regular cells are unchanged.

The framing is a novelty showcase: ironpad's public notebooks are its marketing,
and "this cell thinks it is Linux" is a striking thing to open a tab to.

# Problem

ironpad cells target `wasm32-unknown-unknown` in a Web Worker. There is no
filesystem, no process, no socket, no real thread. That is correct for what
cells are, and it is also a hard ceiling on what a notebook can demonstrate.

BrowserPod 3.0 added a Rust target that lifts the ceiling, for programs willing
to be Linux processes instead of wasm modules.

# Technical Approach

## What the spike established (2026-08-15)

A program using `std::fs`, `std::thread`, `std::process::Command`, `std::env`
and `std::time` compiled **first try, no source changes, in 0.99s**, to a 544KB
binary exporting `_start`, `main`, `__wasm_call_ctors` and `memory`.

Install is ironpad's own mechanism: a checksum-pinned 38MB tarball drops a
rustup toolchain into `~/.rustup/toolchains/browserpod-3.0.0`, pinning
`nightly-2026-05-19` beneath it. `rustup toolchain uninstall` reverses it.

**The import table is the architectural finding:**

| module | imports |
| --- | --- |
| `i` | **431** (`__syscall_openat_4`, `__syscall_futex_6`, `__syscall_getdents64`, …) |
| `wasi_snapshot_preview1` | 4 (args and environ only) |

Our executor provides roughly a dozen host functions under `env`. 431 against 12
is why this cannot be a build flag on the existing path: it needs their kernel.

## Constraints read off the target spec and the SDK

- **`panic-strategy: abort`.** Our panic hook reports cell errors; here a panic
  takes the pod down (T-007).
- **`--shared-memory` and `+atomics`** mean cross-origin isolation, so
  BrowserPod cells inherit the rayon constraint exactly: no embeds (T-006).
- **`target-family: ["unix"]` and `arch: "wasm64"`** are deliberate
  misdirections in their spec so crates do not take reduced-functionality paths.
  Both are load-bearing and neither is our business, but they explain why
  unmodified crates compile.
- **The runtime is CDN-only and cannot be vendored.** The `browserpod` npm
  package is a 7KB shim whose entire body is
  `import("https://rt.browserpod.io/3.0.1/browserpod.js")`. Their uptime becomes
  ironpad's uptime for these cells, and offline is impossible. The BUILD
  toolchain, by contrast, is a tarball we can vendor (T-008).
- **`apiKey` is required, not optional**, per `index.d.ts`. It ships to the
  client because it is a browser runtime; a console domain lock is the only real
  protection.

## Shape

```
regular cell   source -> CELL_TOOLCHAIN  -> wasm32-unknown-unknown -> executor -> cell_main -> typed panels
browserpod     source -> BROWSERPOD_TC   -> wasm32-browserpod-...  -> Pod      -> _start    -> stdout text
```

Two pipelines that share the compile-cache and admission machinery and share
nothing at runtime.

# Assumptions

- ironpad qualifies for free use: it is public and MIT, and their README says
  BrowserPod is "free to use only for personal and open-source projects".
- Traffic is negligible, so metering is a rounding error in practice. This is a
  reason to proceed, not a reason to skip T-000.

# Constraints

- **Nothing ships to `/public` before T-000.** The free-tier token limits are
  not published; the pricing policy linked from their own README returns 404.
- **No detection.** Flipping a cell to a different runtime because someone typed
  `std::fs` would silently change its semantics and its piping behaviour.
- Free-tier attribution (visible link + logo) is a requirement, not a courtesy.

# Open Questions

1. **Does per-notebook attribution satisfy "your application must display"?**
   Rendering it only where BrowserPod is used is arguably more honest than a
   blanket footer, but it is an interpretation of their terms. Ask them.
2. **What are the free-tier token limits, and what is the metering unit?**
   Unanswerable from public docs today.
3. **What does `capture-outputs` do with a BrowserPod cell?** It runs cells to
   snapshot output, so it would need the runtime and the key in the capture
   environment. If it cannot, the public notebook ships with no `saved_output`,
   which is a visible downgrade on view-only pages.
4. **What does a BrowserPod cell demonstrate that is worth a notebook?** A
   filesystem walk is a shrug. Something like `ripgrep` over a seeded tree, or a
   subprocess pipeline, earns the tab. Decide before building T-009.
5. **Does the CSP need widening** for the dynamic import from `rt.browserpod.io`?
   The current policy sets `object-src`/`base-uri`/`form-action` and
   deliberately no `script-src`, so it may already pass. Verify, do not assume.

# Non-Goals (MVP)

- Compiling Rust in the browser. Their post says it is planned and not
  implemented; if it lands, revisit the whole design.
- Typed piping into or out of BrowserPod cells.
- Running BrowserPod cells in embeds.
- Self-hosting the runtime (enterprise contact only).
- Making regular cells use this path.

# History

- 2026-08-15: Created after a spike that compiled a beefy program to the target
  on the first attempt. Explicit-opt-in decided (owner agreed); 431-syscall
  import table measured; CDN-only runtime and required API key confirmed from
  the SDK's own type definitions. Blocked on T-000.
