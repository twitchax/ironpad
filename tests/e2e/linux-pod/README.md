# Pod-booting e2e specs (PRD-0066)

Specs in this directory boot a **real BrowserPod pod**. Nothing else in
`tests/e2e/` may.

## Why this directory exists

A pod boot costs **10 tokens, flat**, out of a **~1,000-boot monthly
allowance** — measured, and duration-independent: a pod held idle for 322
seconds cost exactly the same as one that lived for a second. Booting pulls
five assets from `rt.browserpod.io` and then never contacts their origin
again, which is why holding a pod is free and creating one is the entire cost.

A pod-dependent spec run is roughly 40 tokens. Ten gate runs a day is 400/day,
and the month is gone in under four weeks with no users involved. **The test
suite, not visitors, is the dominant consumer.** So the gate does not run
these.

## Running them

```bash
cargo make test-linux-cells          # every spec here
cargo make test-linux-cells -- -g "shared filesystem"
```

`cargo make uat` and CI never do — and cannot: the default Playwright project
both ignores this directory and launches Chromium with `rt.browserpod.io`
mapped to `~NOTFOUND`, so a spec that ended up in the wrong place fails on an
unreachable host rather than quietly spending the allowance.
`tests/e2e/linux-cells.spec.ts` asserts that block is live.

## What does NOT belong here

Anything that can make its point without a pod, which is most of it:

- **Asserting absence** (no CDN contact, nothing autoruns, embeds refuse
  Linux cells) boots nothing by definition. Those live in
  `tests/e2e/linux-cells.spec.ts` and run in the gate.
- **CDN failure** is tested by routing `rt.browserpod.io` to a failure with
  `blockPodCdn()` from `tests/e2e/helpers/browserpod.ts` — cheaper and more
  deterministic than booting a pod and killing it.
- **Server-side compilation** of a Linux cell is a `cargo make
  test-integration` concern; it produces a binary and runs nothing.

Put a spec here only when the assertion is genuinely about a running Linux
process: output streaming, the shared filesystem between two cells, real
threads, subprocesses.
