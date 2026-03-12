---
id: PRD-0009
title: "Release Readiness: CI, Docker, Coverage, Fly.io, README"
status: draft
owner: "Aaron Roney"
created: 2026-03-12
updated: 2026-03-12

depends_on:
- PRD-0008

principles:
- "Docker is the sole distribution artifact — no standalone binary"
- "Follow existing CI patterns from kord/ratrod repos"
- "Stable Rust toolchain (no nightly)"
- "Fly.io config is gitignored (.hidden/fly.toml)"
- "Keep cargo-make as the local task runner; CI calls cargo-make tasks"

references:
- name: "kord CI workflow"
  url: https://github.com/twitchax/kord/blob/main/.github/workflows/build.yml
- name: "ratrod CI workflow"
  url: https://github.com/twitchax/ratrod/blob/main/.github/workflows/build.yml
- name: "kord fly.toml"
  url: https://github.com/twitchax/kord/blob/main/.hidden/fly.toml
- name: "Fly.io Dockerfile reference"
  url: https://fly.io/docs/reference/builders/#dockerfile

acceptance_tests:
- id: uat-001
  name: "CI workflow runs fmt-check, clippy, and tests on push"
  command: "gh workflow run build.yml --ref main"
  uat_status: unverified
- id: uat-002
  name: "Code coverage report uploads to Codecov"
  command: "cargo make coverage"
  uat_status: unverified
- id: uat-003
  name: "Docker image builds and pushes to ghcr.io/twitchax/ironpad on main"
  command: "docker build -f docker/Dockerfile -t ironpad ."
  uat_status: unverified
- id: uat-004
  name: "Fly.io config is valid and deployable"
  command: "fly deploy --config .hidden/fly.toml --dry-run"
  uat_status: unverified
- id: uat-005
  name: "README has badges, quick-start, and Docker run instructions"
  command: "head -20 README.md"
  uat_status: unverified

tasks:
- id: T-001
  title: "Create GitHub Actions CI workflow"
  priority: 1
  status: todo
  notes: "Create .github/workflows/build.yml. Trigger: on [push]. Jobs: (1) test — checkout, dtolnay/rust-toolchain@stable, rust-cache, install cargo-make + cargo-nextest + wasm32 target, run cargo make ci. (2) codecov — needs test, install cargo-llvm-cov, run coverage, upload via codecov/codecov-action@v5 with CODECOV_TOKEN secret."

- id: T-002
  title: "Add Docker build + push job to CI workflow"
  priority: 1
  status: todo
  notes: "Add docker job to build.yml. Condition: if github.ref == 'refs/heads/main'. Steps: checkout, login to ghcr.io via docker/login-action, build with docker/build-push-action, push to ghcr.io/twitchax/ironpad:latest + ghcr.io/twitchax/ironpad:sha-$GITHUB_SHA. Use buildx for layer caching."

- id: T-003
  title: "Add cargo make coverage task"
  priority: 1
  status: todo
  notes: "Add coverage task to Makefile.toml. Installs cargo-llvm-cov if missing, runs cargo llvm-cov nextest --workspace --lcov --output-path coverage.lcov. Also add install-coverage-tools dependency task."

- id: T-004
  title: "Create Fly.io deployment config"
  priority: 2
  status: todo
  notes: "Create .hidden/fly.toml with app name twitchax-ironpad, region sea, internal_port 3111, force_https, auto_stop_machines stop, auto_start_machines true, min_machines_running 0. Dockerfile reference: docker/Dockerfile. Add .hidden/ to .gitignore. Include persistent volume mount for /data and /cache."

- id: T-005
  title: "Update README with badges and quick-start"
  priority: 2
  status: todo
  notes: "Add badges: CI status (GitHub Actions), codecov coverage, license. Add Quick Start section with Docker run command (docker run -p 3111:3111 ghcr.io/twitchax/ironpad:latest). Keep existing architecture docs intact."

- id: T-006
  title: "Add .gitignore entries for new artifacts"
  priority: 1
  status: todo
  notes: "Ensure .gitignore includes: .hidden/, coverage.lcov, *.lcov. Check existing .gitignore and only add missing entries."

- id: T-007
  title: "Add LICENSE file"
  priority: 2
  status: todo
  notes: "Add MIT LICENSE file if not already present. Reference in README badges."

- id: T-008
  title: "Optimize Dockerfile for CI caching"
  priority: 3
  status: todo
  notes: "Consider adding DOCKER_BUILDKIT=1 inline cache hints to Dockerfile for faster CI rebuilds. Add .dockerignore to exclude target/, node_modules/, test-results/, playwright-report/ from build context."
---

# Summary

Prepare ironpad for public release by adding GitHub Actions CI/CD, code coverage reporting, Docker image publishing to GHCR, Fly.io deployment configuration, and README polish.

# Problem

ironpad currently has no automated CI — all quality gates (fmt, clippy, test) run locally via `cargo make`. There's no published Docker image, no code coverage tracking, no deployment config, and the README lacks badges and quick-start instructions. This blocks sharing the project publicly and deploying it.

# Goals

1. Every push triggers CI (fmt-check, clippy, tests) via GitHub Actions
2. Code coverage tracked via Codecov with `cargo-llvm-cov`
3. Docker image auto-published to `ghcr.io/twitchax/ironpad` on main pushes
4. Fly.io config ready for `fly deploy` (gitignored, not committed)
5. README has badges, quick-start Docker instructions, and license info
6. Local developer experience preserved — `cargo make` remains the primary interface

# Technical Approach

## CI Workflow (`.github/workflows/build.yml`)

Single workflow file, triggered on all pushes, with three jobs:

```
push → test (fmt + clippy + test)
         ↓
       codecov (coverage → Codecov)
         ↓
       docker (build + push to GHCR, main only)
```

**Test job**: Uses `dtolnay/rust-toolchain@stable`, `Swatinem/rust-cache@v2`, installs `cargo-make` and `cargo-nextest`, runs `cargo make ci`. This ensures CI runs the same checks as local dev.

**Codecov job**: Installs `cargo-llvm-cov`, generates LCOV report, uploads via `codecov/codecov-action@v5`. Requires `CODECOV_TOKEN` repository secret.

**Docker job**: Uses `docker/build-push-action` with buildx. Tags: `latest` + `sha-<commit>`. Pushes to `ghcr.io/twitchax/ironpad`. Only runs on main branch.

## Coverage (`Makefile.toml`)

New `coverage` task wrapping `cargo llvm-cov nextest`. Generates LCOV for CI upload and local inspection.

## Fly.io (`.hidden/fly.toml`)

Follows kord pattern: gitignored config in `.hidden/`. App: `twitchax-ironpad`, region: `sea`, port 3111, auto-scaling (0 min machines), HTTPS forced. Persistent volumes for `/data` (shares, public notebooks) and `/cache` (compiled cell WASM).

## README

Add badges block at top, Docker quick-start section, keep existing architecture documentation.

# Assumptions

- `CODECOV_TOKEN` secret will be configured in the GitHub repo settings
- `GITHUB_TOKEN` (default) has permissions to push to GHCR
- Fly.io CLI (`flyctl`) is installed locally for deployment
- The existing Dockerfile works correctly for CI builds

# Constraints

- ironpad requires a full Rust toolchain at runtime (for user cell compilation), making the Docker image large (~2GB+). This is inherent to the architecture.
- CI build times will be significant due to cargo-leptos compilation + WASM target. Rust cache should mitigate repeat builds.
- No Windows/macOS builds — Docker is the sole artifact.

# References to Code

- `docker/Dockerfile` — Existing multi-stage build (builder + runtime with Rust toolchain)
- `docker/docker-compose.yml` — Local development compose file
- `Makefile.toml` — All cargo-make tasks (ci, test, clippy, fmt-check, build, docker-build)
- `Cargo.toml` — Workspace config, cargo-leptos metadata (site addr 127.0.0.1:3111)
- `crates/ironpad-server/` — Server binary entry point

# Non-Goals (MVP)

- Automated Fly.io deployment from CI (deploy manually with `fly deploy`)
- GitHub Releases with changelogs
- Multi-arch Docker images (amd64 only for now)
- Integration/Playwright tests in CI (can add later — requires browser + running server)
- Semantic versioning automation

# History

(Entries appended during implementation go below this line.)
