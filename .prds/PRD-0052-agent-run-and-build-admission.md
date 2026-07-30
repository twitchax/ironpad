---
id: PRD-0052
title: "Agent-triggered execution and build admission control"
status: done
owner: "Aaron Roney"
created: 2026-07-30
updated: 2026-07-30

depends_on:
- PRD-0038
- PRD-0045

principles:
- "Execution is an editor action, not a state mutation. CellRun rides the Mutation envelope for its permission gate, but the browser dispatches it to the run queue, never through model.apply."
- "The protocol's execution events were reserved, not wired. CellCompiling/CellCompiled/CellExecuted are emitted only while a session is active, so no stale backlog flushes at the next session start."
- "Admission control protects the scarce resource, which is a cargo process, not an HTTP request. Cache hits are never rate limited; builds are."
- "A live check that cannot get a slot degrades to Skipped, the status the client already retries. Typing never blocks on capacity."
- "One agent waiting on a run needs a terminal signal on every path: executed, compile failed, or execution failed. Timeouts are for lost peers, not expected outcomes."

references:
- name: "tokio Semaphore"
  url: https://docs.rs/tokio/latest/tokio/sync/struct.Semaphore.html
- name: "Fly-Client-IP header"
  url: https://fly.io/docs/networking/request-headers/

acceptance_tests:
- id: uat-001
  name: "CellRun round-trips the protocol and an old peer degrades it to Mutation::Unknown"
  command: cargo make test
  uat_status: verified
- id: uat-002
  name: "emit_event buffers a browser-attributed envelope and bumps the generation the bridge drains on"
  command: cargo make test
  uat_status: verified
- id: uat-003
  name: "ironpad cells run executes a cell end-to-end and prints its output"
  command: npx playwright test session
  uat_status: verified
- id: uat-004
  name: "A second concurrent compile of a distinct cell queues behind the build semaphore and completes"
  command: cargo make test
  uat_status: verified
- id: uat-005
  name: "A live check with no free slot returns Skipped instead of queueing"
  command: cargo make test
  uat_status: verified
- id: uat-006
  name: "The per-IP bucket rejects a burst of builds with a clear error and never counts cache hits"
  command: cargo make test
  uat_status: verified

tasks:
- id: T-001
  title: "Protocol: Mutation::CellRun, MutationResult::CellRunStarted, CellExecuted.success, PROTOCOL_VERSION 3"
  priority: 1
  status: done
  notes: "success defaults to true so a legacy CellExecuted without the field parses as the old success-only semantics."
- id: T-002
  title: "Model: emit_event for browser-originated execution events"
  priority: 1
  status: done
  notes: "Same pending_events push + generation bump as apply, without a mutation."
- id: T-003
  title: "Browser: handle CellRun via the run queue and emit execution events from the compile flow"
  priority: 1
  status: done
  notes: "connection.rs intercepts CellRun before model.apply; state.rs grows request_cell_run (validate runnable, append to run_all_queue). cell_item emits events gated on session.active."
- id: T-004
  title: "CLI: cells run subcommand with event wait"
  priority: 1
  status: done
  notes: "DaemonState grows a broadcast channel fed by handle_ws_message. serve_cells_run subscribes BEFORE sending, then waits for CellExecuted or CellCompiled{success:false} for the cell."
- id: T-005
  title: "Build admission: global compile semaphore + check try_acquire + per-IP token bucket"
  priority: 1
  status: done
  notes: "BuildPermits context alongside CompileLocks. Compile waits up to a bounded queue timeout; checks Skip; the bucket runs after the cache lookup so hits are free."
- id: T-006
  title: "Config knobs + spans"
  priority: 2
  status: done
  notes: "--max-concurrent-builds / IRONPAD_MAX_CONCURRENT_BUILDS (default 3); env-tunable bucket and queue timeout following the IRONPAD_BUILD_TIMEOUT_SECS precedent. build_permit_wait span."
- id: T-007
  title: "e2e: agent runs a cell over the session and reads its output"
  priority: 2
  status: done
  notes: "Extends the existing agent-session Playwright spec; asserts both CLI stdout and browser output."
- id: T-008
  title: "Docs: DEVELOPMENT.md collaboration + observability sections, CLAUDE.md"
  priority: 3
  status: done
  notes: ""
---

# Summary

Closes the agent collaboration loop: an agent can now run a cell and read its
result, not just edit source and hope. Pairs that new capability with admission
control on the compile pipeline, since handing agents a run button without
bounding concurrent cargo processes would be an invitation.

# Problem

Two halves:

1. The collaboration protocol is CRUD-only. An agent edits a cell over the
   WebSocket, and then a human must click Run. The execution status events
   (`CellCompiling`, `CellCompiled`, `CellExecuted`) have existed in the
   protocol since PRD-0038 and the CLI daemon already pattern-matches them,
   but the browser never emits them: they were reserved, never wired.

2. Nothing bounds concurrent builds. The per-cell locks serialize same-cell
   compiles only; N distinct cell_ids buy N concurrent `cargo build`s (each up
   to the 300s timeout) on one Fly machine. A classroom on an unwarmed
   notebook does this by accident; a script does it on purpose.

# Goals

1. `ironpad cells run <cell_id>` executes a cell in the hosting browser and
   prints its output or its diagnostics.
2. Execution status flows to every session guest as events.
3. At most N cargo processes run concurrently (default 3), with queueing for
   compiles and shed-load Skipped for live checks.
4. Per-IP limits on build starts, not on requests. Cache hits stay free.

# Technical Approach

## CellRun rides Mutation, dispatches to the run queue

`Mutation::CellRun { cell_id }` gets the relay's existing write-permission
gate for free. On the browser it is intercepted BEFORE `model.apply`:
execution does not change notebook state, so it must not enter the state
machine. The handler validates the cell is runnable, appends to
`run_all_queue` (the same queue Run All and cascading execution use, so
prerequisite cascading just works), and acks with
`MutationResult::CellRunStarted`. Results arrive as events, correlated by
cell_id rather than message id, because one run can fan out into a cascade.

`Event::CellExecuted` gains `success: bool` defaulting to true: an agent
waiting on a run needs a terminal event for execution failure, and the old
event only existed on the success path. `PROTOCOL_VERSION` bumps to 3.
Forward-compat is already structural: every protocol enum carries a
`#[serde(other)] Unknown` arm, so old peers degrade instead of hanging.

## Event emission is session-gated

`NotebookModel::emit_event` pushes a browser-originated envelope into the
same buffer `apply` uses. The compile flow in `cell_item.rs` calls it at
compile start, compile end (with diagnostics), and execution end, but only
while `session.active` — otherwise events would pile into the buffer and
flush as a stale backlog the moment a session starts.

## The daemon waits on a broadcast, not a request id

`DaemonState` grows a `tokio::sync::broadcast` of incoming event envelopes.
`cells.run` subscribes before sending (a cache-hit compile can finish fast
enough to race a late subscription), sends the mutation, confirms the
`CellRunStarted` ack, then waits for the first `CellExecuted` or
`CellCompiled { success: false }` matching its cell, up to `--timeout-secs`
(default 360, sized to the 300s build timeout).

## Admission control

`BuildPermits`, a context alongside `CompileLocks`, holds two semaphores
sized by `--max-concurrent-builds` (default 3):

- **Compiles** acquire with a bounded queue timeout; exhaustion returns a
  clear "at capacity" error rather than a socket held open indefinitely.
- **Live checks** `try_acquire` and return `CheckStatus::Skipped` on
  exhaustion — the client already treats Skipped as "try again after the
  next quiet period", so typing never blocks on capacity.

The per-IP token bucket sits AFTER the cache lookup in `compile_cell_core`,
so it prices what actually costs something: a cargo spawn. IP comes from
`Fly-Client-IP` (then `X-Forwarded-For`, then a shared local key). The
bucket is a plain HashMap + refill-on-access under a Mutex; no dependency.

# Assumptions

- One server process (the semaphores and buckets are in-process state, like
  the compile locks and OG render locks before them).
- The browser hosting the session stays open; a run against a closed host
  times out at the daemon, which is the existing failure mode for every
  forwarded command.

# Constraints

- The wire format must stay compatible: a v2 CLI against a v3 browser (and
  the reverse) must degrade, not hang. The Unknown arms carry this.
- The rate limiter must not fire during Playwright runs, whose compiles are
  overwhelmingly cache hits; limits are env-tunable for the capacity tests.

# References to Code

- `crates/ironpad-common/src/protocol.rs` — CellRun, CellRunStarted, CellExecuted.success
- `crates/ironpad-app/src/model.rs` — emit_event
- `crates/ironpad-app/src/session/connection.rs` — CellRun interception
- `crates/ironpad-app/src/pages/notebook_editor/state.rs` — request_cell_run
- `crates/ironpad-app/src/pages/notebook_editor/cell_item.rs` — event emission
- `crates/ironpad-app/src/compiler/mod.rs` — BuildPermits
- `crates/ironpad-app/src/server_fns.rs` — admission in compile/check cores
- `crates/ironpad-cli/src/daemon.rs`, `main.rs` — cells.run
- `crates/ironpad-server/src/config.rs`, `main.rs` — the knob + context

# Non-Goals (MVP)

- Cancelling an in-flight run from the CLI (the browser's Stop remains the
  only cancel path).
- Streaming partial output; the CLI prints terminal results only.
- Distributed rate limiting or persistence of buckets across restarts.
- Running cells without a hosting browser (headless execution is a different
  product).

# History

- 2026-07-30: Created.
- 2026-07-30: Implemented and verified (767 unit / 12 integration / 96 e2e).
  Two findings from the e2e pass are recorded as design notes: context lookup
  (`expect_context`) inside a WebSocket callback has no reliable reactive
  owner, so `NotebookState` is captured at `start_session` under the page's
  owner and moved into the message closure (this also hardened the
  pre-existing persist-after-agent-edit path); and the Playwright suite's
  deliberate always-miss compiles (failed compiles are never cached, Force
  Recompile bypasses every layer) share one "local" bucket, so the suite
  overrides the rate env vars and the production defaults were resized for
  the human error-iteration loop (burst 20, 30/min).
