# developer-workflow Specification

## Purpose
The maintainer contract for the ironpad repository: the cargo-make tasks that build, serve, and test it, what each gate enforces and how it reports a failure, and what the GitHub Actions workflow runs on pushes and pull requests. A maintainer can run every gate locally and see it pass or fail.

## Requirements
### Requirement: The ci gate
`cargo make ci` SHALL run `fmt-check`, `gen-completions-check`, `capture-outputs-check`, `glyph-check`, `css-vars-check`, `clippy`, `test`, and `test-js`, and SHALL fail when any of them fails. (PRD-0001)

#### Scenario: Unformatted code
- **WHEN** a Rust file is not `rustfmt`-clean
- **THEN** `cargo make ci` fails in `fmt-check` (`cargo fmt --all --check`) without changing the file

### Requirement: The uat gate
`cargo make uat` SHALL run `ci`, then `test-integration`, then `playwright`, and SHALL never boot a metered BrowserPod pod. It is the full local gate. (PRD-0001, PRD-0066)

#### Scenario: Full gate
- **WHEN** a maintainer runs `cargo make uat`
- **THEN** a failure in any of the three tiers fails the run
- **AND** no spec under `tests/e2e/linux-pod/` runs

### Requirement: Clippy gate
`cargo make clippy` SHALL run `cargo clippy --all-targets -- -D warnings` under the workspace lint table, which enables `clippy::pedantic` apart from a handful of allowed style lints, so any warning in any target, tests included, fails the gate. (PRD-0012)

#### Scenario: Pedantic lint in a test
- **WHEN** a test module introduces an `explicit_counter_loop`
- **THEN** `cargo make clippy` fails

### Requirement: Unit and integration test tiers
`cargo make test` SHALL run the workspace's tests with cargo-nextest, skipping `#[ignore]`d tests, and `cargo make test-integration` SHALL run only the ignored ones: slow tests that compile real cells and need the wasm32 target and the pinned cell toolchains. Both SHALL pass when no test matches. (PRD-0001, PRD-0038)

#### Scenario: Slow tests stay out of ci
- **WHEN** a maintainer runs `cargo make test`
- **THEN** the compiler-pipeline tests that build real cells do not run
- **AND** `cargo make test-integration` runs exactly those

### Requirement: JavaScript unit tests
`cargo make test-js` SHALL run `node --test tests/js/*.test.mjs`, exercising the BrowserPod runtime and the cell executor scripts against fakes with no browser, no network, and no pod boot. (PRD-0066)

#### Scenario: Executor regression
- **WHEN** a change to the executor scripts breaks a case the harness covers
- **THEN** `cargo make test-js`, and therefore `cargo make ci`, fails

### Requirement: Playwright gate
`cargo make playwright` SHALL build `ironpad-cli` in release mode, then run the `chromium` Playwright project against `cargo leptos serve --release` on port 3111, reusing a running server outside CI, and pass arguments after `--` through as a filter. That project SHALL skip `tests/e2e/linux-pod/` and make the BrowserPod CDN unresolvable. (PRD-0066, d3a051a agentic-collaboration/0011)

#### Scenario: Filtered run
- **WHEN** a maintainer runs `cargo make playwright -- session`
- **THEN** only the specs matching `session` run, using the freshly built CLI binary

#### Scenario: Test server configuration
- **WHEN** Playwright starts the server itself
- **THEN** it sets `IRONPAD_TEST_AUTH=1`, an admin login, placeholder OAuth credentials, a fake `BROWSERPOD_KEY` unless the environment supplies one, a 300-second live-check timeout, and build rate limits high enough for the suite

### Requirement: Opt-in pod-booting specs
`cargo make test-linux-cells` SHALL run only the specs under `tests/e2e/linux-pod/`, through a `linux-pod` Playwright project that exists only when `IRONPAD_LINUX_POD_TESTS` is set, and SHALL exit 1 unless a real `BROWSERPOD_KEY` comes from the environment or `.hidden/dev.env`. Neither `ci`, `uat`, nor the CI workflow runs it. (PRD-0066)

#### Scenario: No key
- **WHEN** a maintainer runs `cargo make test-linux-cells` with no `BROWSERPOD_KEY` and no `.hidden/dev.env`
- **THEN** it prints that the key is unset and exits 1 without booting a pod

### Requirement: Completions index freshness
`cargo make gen-completions-check` SHALL fail when the committed `public/monaco/completions-index.json` differs from what `tools/gen-completions.py` generates from `ironpad-cell`'s public items; `cargo make gen-completions` SHALL regenerate it. (PRD-0045 T-005)

#### Scenario: New cell API without a regenerated index
- **WHEN** a `pub fn` is added to `ironpad-cell` and the index is not regenerated
- **THEN** `cargo make gen-completions-check` exits 1

### Requirement: Glyph guard
`cargo make glyph-check` SHALL fail, printing `<path>:<line>: bare glyph … -> use an icons:: role`, when a non-comment line of a crate's `src/` (before its first `#[cfg(test)]`) or of a non-vendored `public/*.js` renders a Unicode symbol glyph, literally or as a `\u` escape. A line commented `glyph-check: allow` is exempt. (PRD-0062)

#### Scenario: Glyph in escape form
- **WHEN** a component renders `"\u{2573} Delete"` as a button label
- **THEN** `cargo make glyph-check` reports that line and exits 1

### Requirement: CSS custom property guard
`cargo make css-vars-check` SHALL fail, printing `style/main.scss:<line>: <name> is read but never defined`, when the stylesheet reads `var(--name)` with no fallback and never declares `--name`; a read with a fallback, `var(--name, value)`, passes. (c8b9ec1)

#### Scenario: Undefined palette name
- **WHEN** a rule uses `background: var(--ip-bg)` and `--ip-bg` is declared nowhere in `style/main.scss`
- **THEN** `cargo make css-vars-check` reports it and exits 1

### Requirement: Saved-output freshness gate
`cargo make capture-outputs-check` SHALL fail when a public notebook's compiled text no longer matches `public/notebooks/.capture-manifest.json`, when the manifest names a deleted notebook, or when a Linux cell carries a saved output. `cargo make capture-outputs`, run against a server on port 3111, SHALL refresh notebooks and manifest, optionally for names given after `--`. (c9a7660)

#### Scenario: Edited showcase cell
- **WHEN** a maintainer edits a code cell of a public notebook without recapturing
- **THEN** `cargo make ci` fails with `stale saved outputs: <name> (compiled text changed since last capture)`

### Requirement: Toolchain pin guards
`cargo make test` SHALL fail when the cell toolchain pin differs between `rust-toolchain.toml`, the toolchain `docker/Dockerfile` installs and defaults to, and `.github/workflows/build.yml`, or when any `wasm-bindgen-cli` pin in the Dockerfile, the workflow, or `Makefile.toml` differs from the `wasm-bindgen` version in `Cargo.lock`. (PRD-0067)

#### Scenario: Lockfile bump without the CLI pins
- **WHEN** `Cargo.lock` moves `wasm-bindgen` to a new version and `Makefile.toml` still installs the old CLI
- **THEN** the test fails with `Makefile.toml does not install wasm-bindgen-cli <locked version> (the Cargo.lock version)`

### Requirement: CI test job
`.github/workflows/build.yml` SHALL run on pushes to `main` and on pull requests, cancelling superseded runs of the same ref. Its Test job SHALL install the pinned nightly with `enzyme` and the BrowserPod toolchain, then run `cargo make ci`, `cargo make test-integration`, and a hydrate build of `ironpad-app` for `wasm32-unknown-unknown`. Playwright does not run in CI. (PRD-0009, PRD-0038)

#### Scenario: Pull request
- **WHEN** a pull request is opened or updated
- **THEN** the Test, Security Audit, and Code Coverage jobs run and the Docker job does not

### Requirement: Build notification carries commit text as data
On pushes, the Test job's final Notify step SHALL receive the commit message, author and job status only through `env:` and format them with `printf`, never through a `${{ }}` expression inside its script, and SHALL be `continue-on-error` so a failed notification cannot fail the job. `cargo make test` SHALL fail when any `run:` block in `build.yml` contains a `${{ }}` expression or a `github.event.head_commit` value is read anywhere but an uppercase env entry.

#### Scenario: A commit message with shell syntax
- **WHEN** a pushed commit's message contains an unmatched backtick, `$(...)`, or a line reading `EOF`
- **THEN** the notification carries that text literally and the Test job's result is unaffected

### Requirement: CI audit, coverage, and image jobs
The workflow SHALL also run a Security Audit job (`cargo audit` over `Cargo.lock`), a Code Coverage job after Test that uploads `cargo llvm-cov nextest` lcov output to Codecov, and, for pushes to `main` after Test passes, a Docker job that builds `docker/Dockerfile` and pushes `ghcr.io/twitchax/ironpad:latest` and `:sha-<commit>`. (PRD-0009, PRD-0038)

#### Scenario: Merge to main
- **WHEN** a commit lands on `main` and the Test job passes
- **THEN** the image is pushed under both tags

### Requirement: Local coverage
`cargo make coverage` SHALL write `coverage.lcov` from `cargo llvm-cov nextest --workspace`, installing cargo-llvm-cov first when it is missing. (PRD-0009)

#### Scenario: First run on a fresh box
- **WHEN** cargo-llvm-cov is not installed and a maintainer runs `cargo make coverage`
- **THEN** it installs cargo-llvm-cov and produces `coverage.lcov`

### Requirement: Docker tasks
`cargo make docker-build` SHALL fetch and checksum-verify the BrowserPod toolchain tarball into `docker/vendor/`, then build `docker/Dockerfile` as `ironpad`, which fetches the `monaco-editor` version `package.json` pins exactly (whatever `public/monaco/vs` the build context holds is replaced) and fails if the site lacks `monaco/vs/loader.js` or any worker file `public/monaco/init.js` names; `docker-up` and `docker-down` SHALL start and stop it through the compose file; `docker-uat` SHALL build, start, wait up to 30 seconds for `http://localhost:3111` to answer, check that it serves `/monaco/vs/loader.js`, check that uid 2000 is refused with a permission error writing `/ironpad/cache/blobs`, reading `/proc/1/environ` and reading `/ironpad/data/ironpad.db/LOCK` (which it first confirms exists as root), run `playwright` with `IRONPAD_DOCKER_UAT=1` so the container-only `build-sandbox.spec.ts` runs, and tear down. (PRD-0001, PRD-0066, PRD-0030)

#### Scenario: Container never answers
- **WHEN** the container does not answer on port 3111 within 30 seconds during `cargo make docker-uat`
- **THEN** it prints `ERROR: Server did not start within 30s.`, stops the container, and exits 1

#### Scenario: The build user reaches the database
- **WHEN** uid 2000 inside the container can read `/ironpad/data/ironpad.db/LOCK`, or a probe fails for any reason other than a permission error
- **THEN** `docker-uat` stops the container and exits 1 before Playwright runs

### Requirement: Dev loop
`cargo make dev` SHALL install Monaco into `public/monaco/` and start `cargo leptos watch`, serving on `http://localhost:3111` with live reload; `cargo make build` SHALL do the same release build once. `cargo make install-tools` SHALL install the tools both need, placing the pinned `wasm-bindgen-cli` in `./.tools`, which every task puts first on `PATH`. (PRD-0001)

#### Scenario: Pinned CLI stays project-local
- **WHEN** a maintainer runs `cargo make install-tools` and then `cargo make dev`
- **THEN** cargo-leptos uses the `wasm-bindgen` in `./.tools/bin`, and nothing is installed into `~/.cargo/bin` for it
