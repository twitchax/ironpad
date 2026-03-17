---
id: PRD-0021
title: "Release Preparation: Test Coverage, README, and Site Promotion"
status: active
owner: "Aaron Roney"
created: 2026-03-17
updated: 2026-03-17

principles:
- "The README should sell the project and show how to use it — not document internals"
- "Follow the kord/rtz README style: badges, one-liner, live link, usage, concise dev section"
- "E2E tests in CI should be practical — a few high-value notebook smoke tests, not an exhaustive suite"
- "No content duplication between README and DEVELOPMENT.md"

references:
- name: "kord README (style reference)"
  url: https://github.com/twitchax/kord
- name: "rtz README (style reference)"
  url: https://github.com/twitchax/rtz
- name: "ironpad live site"
  url: https://ironpad.twitchax.com

acceptance_tests:
- id: uat-001
  name: "Playwright e2e tests run in CI and contribute to code coverage reporting"
  command: cargo make uat
  uat_status: unverified
- id: uat-002
  name: "README is under 150 lines, leads with live site link, and contains no architecture/compiler details"
  command: "grep -c '' README.md && ! grep -qi 'compilation pipeline\\|workspace structure\\|cell I/O' README.md"
  uat_status: unverified
- id: uat-003
  name: "No content duplication between README.md and DEVELOPMENT.md"
  command: "manual review"
  uat_status: unverified

tasks:
- id: T-001
  title: "Audit and fix Playwright e2e tests"
  priority: 1
  status: todo
  notes: "Run existing Playwright suite locally. Identify and fix broken tests. Prune flaky or redundant tests. Goal: a reliable, passing subset."

- id: T-002
  title: "Create focused notebook smoke tests"
  priority: 1
  status: todo
  notes: "Write 2-3 Playwright tests that open complex public notebooks (e.g., reactor sim, fourier, sorting), run them, and verify output renders. These are the high-value e2e tests for CI."

- id: T-003
  title: "Add Playwright e2e tests to CI workflow"
  priority: 2
  status: todo
  notes: "Add a CI job to .github/workflows/build.yml that installs Playwright, builds the app, and runs the e2e suite. Consider whether to include in codecov (cargo-llvm-cov won't capture Playwright — may need separate coverage or just accept e2e as uncovered server-side validation)."

- id: T-004
  title: "Rewrite README in kord/rtz style"
  priority: 2
  status: todo
  notes: "Rewrite README.md to follow the kord/rtz pattern: badges → one-liner with live link to ironpad.twitchax.com → screenshot/demo → Docker quick start → key features → brief development section pointing to DEVELOPMENT.md → license. Target under 150 lines. Remove all architecture, compiler, cell I/O, and workspace structure content."

- id: T-005
  title: "Migrate removed README content to DEVELOPMENT.md"
  priority: 2
  status: todo
  notes: "Move README content that doesn't already exist in DEVELOPMENT.md (e.g., detailed project layout, cell I/O pipeline, FFI layout, Monaco integration details, CLI flags, troubleshooting). Deduplicate — DEVELOPMENT.md already has architecture, compilation, frontend, and agent sections. Only add what's genuinely new."

- id: T-006
  title: "Verify CI passes end-to-end"
  priority: 3
  status: todo
  notes: "Push all changes, verify the full CI pipeline passes including the new Playwright job. Check codecov results."
---

# Summary

Prepare ironpad for public release by improving test coverage through CI-integrated e2e tests, rewriting the README to be user-focused (following the kord/rtz style), and prominently featuring the live site at ironpad.twitchax.com.

# Problem

1. **Test coverage is 42%** — the Playwright e2e tests exercise significant server/app code but don't run in CI or contribute to coverage metrics.
2. **The README is 675 lines** of developer internals (workspace structure, compiler pipeline, cell I/O, FFI layouts). It doesn't communicate what ironpad *is* or why someone should use it. New visitors see implementation details instead of a compelling introduction.
3. **The live site (ironpad.twitchax.com) is not prominently featured** — the best way to understand ironpad is to use it, and the README doesn't direct people there.

# Goals

1. Get Playwright e2e tests running in CI, focusing on a small set of high-value notebook smoke tests
2. Rewrite the README to be concise, user-focused, and compelling — under 150 lines
3. Prominently feature ironpad.twitchax.com as the primary call-to-action
4. Preserve all technical documentation in DEVELOPMENT.md without duplication

# Technical Approach

## Test Coverage (T-001 through T-003)

1. **Audit existing Playwright tests** — run them locally, identify what's broken, fix or prune
2. **Create focused smoke tests** — 2-3 tests that open complex public notebooks, run all cells, and verify output. These exercise the compilation pipeline, WASM execution, and rendering end-to-end.
3. **Add to CI** — new job in `build.yml` that:
   - Installs Rust + wasm target + cargo-leptos
   - Installs Node.js + Playwright browsers
   - Builds the app (`cargo leptos build --release`)
   - Runs Playwright tests
   - Note: Playwright won't directly increase `cargo-llvm-cov` numbers (it tests through HTTP, not Rust calls). But it validates the full stack in CI, which is the real goal.

## README Rewrite (T-004 through T-005)

**New README structure** (following kord/rtz pattern):

```
# ironpad
Badges (build, coverage, license)

One-liner description + link to ironpad.twitchax.com

## Try It Now
Link to live site + brief description of what you'll find

## Features
Bullet list of key capabilities

## Quick Start (Docker)
Docker run command (already exists, keep it)

## Development
2-3 lines + link to DEVELOPMENT.md

## License
MIT
```

**Content migration**: Move removed sections to DEVELOPMENT.md, deduplicating against what's already there. DEVELOPMENT.md already has architecture, compilation pipeline, frontend, and agent sections — so only truly new content (detailed project layout, cell I/O FFI, Monaco details, troubleshooting) needs to move.

# Assumptions

- The live site at ironpad.twitchax.com is stable and publicly accessible
- Playwright tests can run in CI with a reasonable timeout (the webServer config already allows 5 min for cargo build)
- Some existing Playwright tests may be broken and need fixing or removal

# Constraints

- Playwright e2e tests won't increase cargo-llvm-cov coverage numbers (they test via HTTP)
- CI runners need Node.js + Playwright browsers installed — adds CI time
- README must still contain Docker quick start for self-hosting users

# References to Code

- `tests/e2e/` — existing Playwright test suite (1,401 lines across 9 spec files)
- `playwright.config.ts` — Playwright config (uses cargo-leptos serve, port 3111)
- `.github/workflows/build.yml` — CI workflow (currently: test, codecov, docker jobs)
- `README.md` — current 675-line README to rewrite
- `DEVELOPMENT.md` — existing dev guide (destination for migrated content)
- `public/notebooks/` — public notebook `.ironpad` files for smoke tests

# Non-Goals (MVP)

- Achieving a specific coverage percentage number
- Adding unit tests to under-tested modules (separate effort)
- Creating a documentation site or wiki
- Publishing to crates.io
- Adding a CHANGELOG

# History

(Entries appended during implementation go below this line.)

- 2026-03-17: T-001 done — audited Playwright tests: 16 pass, 9 skipped (CLI/filesystem-dependent)
- 2026-03-17: T-002 done — created notebook-smoke.spec.ts with mandelbrot, game-of-life, fourier tests
- 2026-03-17: T-003 done — added Playwright CI job to build.yml
- 2026-03-17: T-004 done — rewrote README to 58 lines (kord/rtz style)
- 2026-03-17: T-005 done — migrated unique README content to DEVELOPMENT.md (615 lines)
- 2026-03-17: T-006 in progress — pushed, CI running (run 23217674237)
