---
id: PRD-0023
title: "Domain-Filtering Forward Proxy for Compilation Security"
status: done
owner: "Aaron Roney"
created: 2026-03-18
updated: 2026-03-18

principles:
- "Defense-in-depth: block network abuse at the proxy layer, not just port-level firewalls"
- "Opt-in: the proxy only activates when IRONPAD_COMPILATION_PROXY is set"
- "Minimal surface: the proxy is a tiny CONNECT-only tunnel, not a full HTTP proxy"
- "No impact on local dev: proxy is a deployment concern, not a development one"

references:
- name: "Fly.io Network Policies"
  url: https://fly.io/docs/machines/guides-examples/network-policies/
- name: "HTTP CONNECT method (RFC 9110)"
  url: https://httpwg.org/specs/rfc9110.html#CONNECT
- name: "Cargo HTTPS_PROXY support"
  url: https://doc.rust-lang.org/cargo/reference/config.html#httpproxy

acceptance_tests:
- id: uat-001
  name: "Proxy allows CONNECT to allowlisted domain"
  command: "cargo test -p ironpad-proxy"
  uat_status: verified
- id: uat-002
  name: "Proxy rejects CONNECT to non-allowlisted domain"
  command: "cargo test -p ironpad-proxy"
  uat_status: verified
- id: uat-003
  name: "Compilation succeeds with proxy enabled (crates.io reachable)"
  command: "cargo make test-integration"
  uat_status: unverified
- id: uat-004
  name: "Proxy is opt-in: compilation works without IRONPAD_COMPILATION_PROXY"
  command: "cargo make test"
  uat_status: verified
- id: uat-005
  name: "cargo make ci passes"
  command: "cargo make ci"
  uat_status: verified

tasks:
- id: T-001
  title: "Create ironpad-proxy crate with domain-filtering CONNECT proxy"
  priority: 1
  status: done
  notes: "New crate at crates/ironpad-proxy/. Implements an HTTP CONNECT proxy that accepts connections on a configurable bind address (default 127.0.0.1:3112). On each CONNECT request, extracts the target hostname and checks it against an allowlist parsed from the IRONPAD_PROXY_ALLOWLIST env var (comma-separated domains, with suffix matching so 'crates.io' also allows 'static.crates.io'). Allowed → bidirectional TCP tunnel. Denied → 403 Forbidden. Use tokio for async I/O. Keep it minimal: no HTTP body proxying, no caching, CONNECT-only. Include unit tests for allowlist parsing and domain matching logic."

- id: T-002
  title: "Add IRONPAD_COMPILATION_PROXY config flag"
  priority: 1
  status: done
  notes: "Add compilation_proxy: Option<String> to AppConfig (ironpad-common/src/config.rs) and CliArgs (ironpad-server/src/config.rs) with #[arg(long, env = \"IRONPAD_COMPILATION_PROXY\")]. Wire through the From<CliArgs> conversion. When set (e.g., 'http://127.0.0.1:3112'), build_micro_crate and check_micro_crate in compiler/build.rs should add .env(\"HTTPS_PROXY\", proxy_url) to the cargo Command. Update compile_cell in server_fns.rs to pass the new config field through."

- id: T-003
  title: "Update Dockerfile to build and run the proxy"
  priority: 2
  status: done
  notes: "In docker/Dockerfile: (1) Add 'cargo build --release -p ironpad-proxy' in the builder stage alongside ironpad-cli. (2) Copy the binary to /app/ironpad-proxy in the runtime stage. (3) Update CMD to start both: use a shell entrypoint that launches ironpad-proxy in the background before exec-ing ironpad-server. (4) Set default env vars: IRONPAD_COMPILATION_PROXY=http://127.0.0.1:3112 and IRONPAD_PROXY_ALLOWLIST with the standard Rust/Cargo domains."

- id: T-004
  title: "Update fly.toml with proxy env vars"
  priority: 2
  status: done
  notes: "In .hidden/fly.toml, add [env] section with IRONPAD_COMPILATION_PROXY=http://127.0.0.1:3112 and IRONPAD_PROXY_ALLOWLIST='crates.io,static.crates.io,index.crates.io,github.com,objects.githubusercontent.com,raw.githubusercontent.com'. These activate the proxy in the Fly deployment."

- id: T-005
  title: "Add proxy documentation and verify CI"
  priority: 3
  status: done
  notes: "Add a 'Compilation Security' section to DEVELOPMENT.md explaining the proxy architecture, how to enable it locally, and the domain allowlist. Run cargo make ci. Push and verify GH Actions passes."
---

# Summary

Add a lightweight domain-filtering forward proxy (`ironpad-proxy`) that restricts which hosts the Rust compiler can reach during user cell compilation. When deployed (e.g., on Fly.io), cargo builds route through this proxy via `HTTPS_PROXY`, and only connections to allowlisted domains (crates.io, github.com, etc.) are permitted. The proxy is opt-in via a config flag and has zero impact on local development.

# Problem

ironpad compiles arbitrary user-provided Rust code server-side, including user-specified Cargo.toml dependencies. Any dependency (or its transitive dependencies) can include a `build.rs` script that executes during compilation with full network access. On Fly.io, this means a malicious `build.rs` could:

- Query the Fly internal network (`*.internal`, `fly-local-6pn`) to discover other services
- Access the metadata service at `169.254.169.254`
- Connect to other apps in the Fly organization's private network

Fly.io's native network policies only filter by IP/port, not by domain — so a port-based egress rule (allow 80/443) blocks internal network access but can't restrict which public hosts are reachable.

# Goals

1. Domain-level filtering of outbound connections during `cargo build`
2. Opt-in activation via a single config flag (`IRONPAD_COMPILATION_PROXY`)
3. Configurable allowlist via env var (`IRONPAD_PROXY_ALLOWLIST`)
4. Zero impact on local development (proxy only activates when configured)
5. Minimal binary — small, auditable, single-purpose

# Technical Approach

## Architecture

```
cargo build  ──HTTPS_PROXY──►  ironpad-proxy (127.0.0.1:3112)
                                     │
                                     ├─ CONNECT crates.io:443  → ✅ tunnel
                                     ├─ CONNECT github.com:443 → ✅ tunnel
                                     └─ CONNECT evil.com:443   → ❌ 403
```

## Proxy Binary (`crates/ironpad-proxy/`)

A tokio-based TCP server that handles HTTP CONNECT requests:

1. Accept TCP connection
2. Read the HTTP request line: `CONNECT host:port HTTP/1.1\r\n`
3. Read and discard remaining headers (until `\r\n\r\n`)
4. Extract hostname, check against allowlist (suffix match)
5. If allowed: connect to target, respond `HTTP/1.1 200 OK\r\n\r\n`, then bidirectionally copy bytes between client ↔ target using `tokio::io::copy_bidirectional`
6. If denied: respond `HTTP/1.1 403 Forbidden\r\n\r\n`, close

**Allowlist parsing:** `IRONPAD_PROXY_ALLOWLIST=crates.io,github.com` → suffix matching so `static.crates.io` matches `crates.io`, `objects.githubusercontent.com` matches `githubusercontent.com`.

**Default allowlist (suggested for Fly deployment):**
- `crates.io` (covers `static.crates.io`, `index.crates.io`)
- `github.com`
- `githubusercontent.com` (covers `objects.githubusercontent.com`, `raw.githubusercontent.com`)

## Config Plumbing (T-002)

```
CliArgs.compilation_proxy  →  AppConfig.compilation_proxy  →  build_micro_crate()
                                                                    │
                                                              .env("HTTPS_PROXY", url)
```

The `HTTPS_PROXY` env var is set **only** on the child `cargo` process — it does not affect the ironpad server itself.

## Dockerfile (T-003)

```dockerfile
# Builder stage: add ironpad-proxy build
RUN cargo build --release -p ironpad-proxy

# Runtime stage: copy binary
COPY --from=builder /build/target/release/ironpad-proxy /app/ironpad-proxy

# Entrypoint: start proxy in background, then exec server
CMD ["/bin/sh", "-c", "/app/ironpad-proxy & exec /app/ironpad-server"]
```

# Assumptions

- Cargo respects `HTTPS_PROXY` for all HTTPS connections (registry, git deps) — this is documented behavior
- The proxy only needs to handle CONNECT (Cargo uses HTTPS for everything, which means CONNECT through a proxy)
- `tokio::io::copy_bidirectional` is sufficient for tunneling (no need to inspect TLS traffic)
- The proxy and server run in the same container with shared localhost

# Constraints

- The proxy cannot inspect TLS traffic (by design) — it only sees the target hostname from the CONNECT request
- Suffix matching means allowing `github.com` also allows `evil-github.com` — but the actual Cargo ecosystem only uses well-known domains. Could use exact-or-subdomain matching (`.github.com` or `github.com`) for more precision.
- The proxy adds a small latency hop for each connection during compilation — negligible for the use case
- `build.rs` scripts that use raw TCP (not HTTPS) won't route through the proxy — but Cargo itself won't invoke them that way; the proxy covers `cargo`'s own fetching, not arbitrary code in `build.rs`

# References to Code

- `crates/ironpad-app/src/compiler/build.rs:79-93` — cargo Command construction (add HTTPS_PROXY here)
- `crates/ironpad-app/src/compiler/build.rs:200-213` — cargo check Command (same change)
- `crates/ironpad-common/src/config.rs:8-13` — AppConfig struct (add compilation_proxy field)
- `crates/ironpad-server/src/config.rs:7-40` — CliArgs + From conversion (add flag)
- `crates/ironpad-app/src/server_fns.rs:22,72-75` — compile_cell config access and build_micro_crate call
- `docker/Dockerfile:27,53-54,76` — binary build, copy, and CMD
- `.hidden/fly.toml` — Fly.io deployment config

# Non-Goals (MVP)

- Full HTTP proxy (GET/POST body proxying) — CONNECT-only is sufficient
- TLS inspection or certificate pinning
- Rate limiting or bandwidth throttling
- Proxy authentication
- Cargo.toml content sanitization (complementary but separate concern)
- Restricting `build.rs` raw TCP connections (out of scope for a proxy approach)

# History

## 2026-03-18 — Batch Execution (T-001 through T-005)
- **Tasks completed**: T-001, T-002, T-003, T-004, T-005
- **Changes**:
  - T-001: Created `crates/ironpad-proxy/` — CONNECT proxy with `DomainAllowlist`, suffix matching, 16 unit+integration tests
  - T-002: Added `compilation_proxy` field to AppConfig/CliArgs, wired HTTPS_PROXY into build.rs + check commands
  - T-003: Updated Dockerfile — builds proxy, copies binary, starts proxy in background before exec-ing server
  - T-004: Added `[env]` section to `.hidden/fly.toml` with proxy config
  - T-005: Added "Compilation Security" section to DEVELOPMENT.md
- **Test results**: 434 pass, 0 fail, 6 skipped — `cargo make ci` clean
- **UATs verified**: uat-001, uat-002, uat-004, uat-005
- **UATs deferred**: uat-003 (requires live compilation with proxy enabled — manual/integration test)
- **Constitution compliance**: No violations
