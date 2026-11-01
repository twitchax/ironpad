# server-operations Specification

## Purpose
Covers what an operator needs to run an ironpad instance: the published Docker image and its `/ironpad` volume, the server's general flags and environment variables with their defaults, where state lives on disk, HTTP caching and security headers, logging and OpenTelemetry export, and boot-time housekeeping. Flags for compile admission, OAuth, the admin login, BrowserPod, the compile proxy, and the public URL belong to the capabilities that use them.

## Requirements
### Requirement: Storage, port, and cell-runtime flags
The server SHALL read `--data-dir` (env `IRONPAD_DATA_DIR`, default `./data`), `--cache-dir` (`IRONPAD_CACHE_DIR`, `./cache`), `--port` (`IRONPAD_PORT`, `3111`), and `--ironpad-cell-path` (`IRONPAD_CELL_PATH`, `./crates/ironpad-cell`, the runtime crate every cell links), a flag taking precedence over its env var, and SHALL listen on `0.0.0.0:<port>`. (PRD-0001)

#### Scenario: Defaults
- **WHEN** the server starts with no flags and none of these env vars set
- **THEN** it uses `./data`, `./cache`, port 3111, and `./crates/ironpad-cell`

#### Scenario: Flags override
- **WHEN** it starts with `--data-dir /tmp/ironpad-data --cache-dir /tmp/ironpad-cache --port 8080 --ironpad-cell-path /opt/ironpad-cell`
- **THEN** it stores data and cache under those paths and serves on port 8080

### Requirement: Agent relay flags
The server SHALL read `--max-guests` (env `IRONPAD_MAX_GUESTS`, default 512), the cap on concurrent agent WebSocket connections, and `--guest-idle-timeout-secs` (env `IRONPAD_GUEST_IDLE_TIMEOUT_SECS`, default 1800), how long an agent connection may stay silent before it is closed. (PRD-0038)

#### Scenario: Tuned relay
- **WHEN** the server starts with `--max-guests 64 --guest-idle-timeout-secs 300`
- **THEN** a 65th concurrent agent connection is refused and an agent silent for five minutes is disconnected

### Requirement: State layout on disk
The server SHALL keep notebook and account state under the data directory (the accounts database `ironpad.db`, immutable shares and their blob snapshots under `shares/`, generated social cards under `og/`) and only rebuildable compile state under the cache directory (`blobs/`, `targets/`, `workspaces/`, `cargo-home/`). It SHALL create the data directory at boot and SHALL NOT start if the database cannot be opened. (PRD-0001, PRD-0053)

#### Scenario: Fresh data directory
- **WHEN** the server starts with `--data-dir` pointing at a path that does not exist
- **THEN** it creates the directory and an `ironpad.db` inside it before serving requests

### Requirement: Docker image
The published image `ghcr.io/twitchax/ironpad` SHALL serve on port 3111 with one volume at `/ironpad`, holding data in `/ironpad/data` and the compile cache in `/ironpad/cache`. Its entrypoint SHALL start the bundled compile proxy and route cell builds through it, then run the server with `RUST_LOG=info`; the image also ships `ironpad-cli` at `/app/ironpad-cli`. (PRD-0001, PRD-0009, PRD-0038)

#### Scenario: Persistent self-hosting
- **WHEN** an operator runs `docker run -p 3111:3111 -v ironpad:/ironpad ghcr.io/twitchax/ironpad:latest`
- **THEN** the app is served at `http://localhost:3111`
- **AND** shares, accounts, and compiled cells survive recreating the container with the same named volume

#### Scenario: No volume
- **WHEN** the image runs without `-v`
- **THEN** the app still serves, with its state on an anonymous `/ironpad` volume

### Requirement: Warm caches seeded into the volume
On start, the image's entrypoint SHALL copy its pre-built default cell target directory and its atomics std sysroot into `/ironpad/cache/targets/` when the volume lacks them, so the first compile and the first live check are warm. If the image carries neither, it SHALL log a warning and start anyway. (PRD-0024, PRD-0045 T-004)

#### Scenario: First boot on an empty volume
- **WHEN** the container starts against a new volume
- **THEN** it logs "Seeding default cell target dir into volume..." and "Seeding atomics sysroot into volume..." before the server starts
- **AND** a second start against the same volume copies nothing

### Requirement: Compose file
`docker/docker-compose.yml` SHALL run the image tagged `ironpad:latest`, publish port 3111, mount a named volume `ironpad` at `/ironpad`, set the data, cache, port, and cell-path variables to the image's values, and restart unless stopped. (PRD-0001)

#### Scenario: Compose runs the locally built image
- **WHEN** an operator runs `docker build -f docker/Dockerfile -t ironpad .` and then `docker compose -f docker/docker-compose.yml up -d`
- **THEN** the container runs that freshly built image rather than a separately cached compose build

### Requirement: Baked toolchain versions
When `IRONPAD_TOOLCHAIN_VERSIONS_FILE` names a readable file whose first two non-blank lines are the raw output of `rustc --version` and `wasm-bindgen --version`, the server SHALL take its toolchain identity from that file instead of running either command; a missing, unreadable, or short file SHALL fall back to probing. The image sets it to `/app/toolchain-versions`. (71f9649)

#### Scenario: Unset outside the image
- **WHEN** the server runs on a dev box without `IRONPAD_TOOLCHAIN_VERSIONS_FILE`
- **THEN** it probes `rustc` and `wasm-bindgen` as usual and compiles produce the same cache keys as with a matching baked file

### Requirement: Cross-origin isolation headers
Every response SHALL carry `Cross-Origin-Opener-Policy: same-origin` and `Cross-Origin-Embedder-Policy: require-corp`, so pages are cross-origin isolated and threaded cells can use `SharedArrayBuffer`. (PRD-0024)

#### Scenario: Any page
- **WHEN** a client fetches `/`
- **THEN** the response carries both headers with those exact values

### Requirement: Content Security Policy
Every response SHALL carry `Content-Security-Policy: object-src 'none'; base-uri 'self'; form-action 'self'`, with no `script-src` and no `frame-ancestors` directive, so the embed routes stay frameable by third parties. (PRD-0051)

#### Scenario: Policy contents
- **WHEN** a client fetches any page
- **THEN** the policy contains `object-src 'none'`, `base-uri 'self'`, and `form-action 'self'`
- **AND** it contains neither `script-src`, `unsafe-inline`, nor `frame-ancestors`

### Requirement: Cross-origin resource policy for embeds
Responses under `/embed/`, and `/embed.js` and `/embed-frame.js`, SHALL also carry `Cross-Origin-Resource-Policy: cross-origin`, so pages that are themselves cross-origin isolated can frame a notebook and load the loader script; no other path gets it. (PRD-0039)

#### Scenario: Embed versus app paths
- **WHEN** a client fetches `/embed/shared/abc123` and `/shared/abc123`
- **THEN** only the first response carries `Cross-Origin-Resource-Policy: cross-origin`
- **AND** `/embedx` and `/api/embed/whatever` do not carry it either

### Requirement: Browser cache policy
Files served from `/pkg/` (content-hashed bundles) and `/share-blobs/` SHALL carry `Cache-Control: public, max-age=31536000, immutable`; every other response that sets no `Cache-Control` of its own SHALL carry `no-cache`, so URL-stable assets and pages revalidate on each use and an old client never runs a stale bundle after a release. (PRD-0047)

#### Scenario: Hashed bundle versus stable script
- **WHEN** a client fetches `/pkg/ironpad.abc123.wasm` and `/executor-bridge.js`
- **THEN** the first is `public, max-age=31536000, immutable` and the second is `no-cache`
- **AND** `/`, `/monaco/vs/loader.js`, `/notebooks/cannon.ironpad`, and `/pkgx/evil.js` are all `no-cache`

### Requirement: Logging
The server SHALL log to stdout through `tracing`, filtered by `RUST_LOG` and defaulting to `info`, and SHALL log its data directory, cache directory, cell-crate path, and listen address at startup. (PRD-0001)

#### Scenario: Default verbosity
- **WHEN** the server starts with `RUST_LOG` unset
- **THEN** info-level lines appear on stdout, including `listening on http://0.0.0.0:3111`

### Requirement: OpenTelemetry export
OTLP export of traces and logs SHALL be off unless `OTEL_EXPORTER_OTLP_ENDPOINT` is set; when set, the server SHALL export over HTTP using the standard `OTEL_EXPORTER_OTLP_*` variables for endpoint and headers, under service name `ironpad-server`, without exporting the exporter stack's own logs. A failed exporter setup SHALL disable export, not the server. (891dbc5)

#### Scenario: Export disabled by default
- **WHEN** the server starts without `OTEL_EXPORTER_OTLP_ENDPOINT`
- **THEN** it logs only to stdout and exports nothing

#### Scenario: Export enabled
- **WHEN** it starts with `OTEL_EXPORTER_OTLP_ENDPOINT=https://otlp-gateway.grafana.net/otlp`
- **THEN** it logs "OpenTelemetry OTLP trace + log export enabled" and exports traces and logs as `ironpad-server`

### Requirement: One span per HTTP request
The server SHALL open one info-level span per HTTP request, named by method and route template (for example `GET /og/{class}/{file}`, or the raw path when no route matched), recording the method and the path but never the query string, which carries the session token on `/ws/connect`. (891dbc5)

#### Scenario: Agent connection trace
- **WHEN** an agent connects to `/ws/connect?token=<token>`
- **THEN** the exported span carries the path `/ws/connect` and no token

### Requirement: Boot-time cache pressure valve
At startup, before serving, the server SHALL check the cache filesystem; when it is at least 80% used and has under 20 GiB free, it SHALL delete `targets/` and `workspaces/`, re-measure, and delete `cargo-home/` only if pressure persists. It SHALL never delete `blobs/`, and an unmeasurable filesystem SHALL skip the check. (b2a9d36)

#### Scenario: Small full volume
- **WHEN** the server boots with its cache on a 5 GB volume that is 85% full
- **THEN** `targets/` and `workspaces/` are gone before the first request and compiled cells in `blobs/` are untouched

#### Scenario: Big disk past the percentage
- **WHEN** the cache volume is 95% used but has hundreds of gigabytes free
- **THEN** nothing is deleted
