# compile-proxy Specification

## Purpose
`ironpad-proxy` is a small CONNECT-only forward proxy that limits which hosts cargo can reach while the server compiles untrusted cells and their dependencies. Operators enable it by running the proxy beside the server, pointing the server's cell builds at it, and naming the allowed domains; it is off by default for local development.

## Requirements
### Requirement: Cell builds route through the proxy when configured
The system SHALL, when `--compilation-proxy` / `IRONPAD_COMPILATION_PROXY` is set, run every cell build and live check's cargo process with `HTTPS_PROXY`, `HTTP_PROXY`, `https_proxy` and `http_proxy` set to that URL and with `NO_PROXY` and `no_proxy` removed, and SHALL set none of them when it is unset (the default), leaving the server's own traffic unproxied either way. (PRD-0023 T-002)

#### Scenario: Proxy configured
- **WHEN** the server starts with `--compilation-proxy http://127.0.0.1:3112`
- **THEN** cargo fetches for cell builds and live checks go through `http://127.0.0.1:3112`

#### Scenario: Proxy unset
- **WHEN** the server starts without a compilation proxy
- **THEN** cell builds reach the network directly and compile as before

### Requirement: CONNECT-only tunnel
The proxy SHALL accept only HTTP `CONNECT host:port` requests and SHALL answer any other request line with `HTTP/1.1 405 Method Not Allowed` and close the connection. (PRD-0023 T-001)

#### Scenario: Plain GET
- **WHEN** a client sends `GET / HTTP/1.1`
- **THEN** the proxy replies `405 Method Not Allowed`

### Requirement: Allowlist configuration
The proxy SHALL read its allowlist from `IRONPAD_PROXY_ALLOWLIST` as comma-separated domains, trimming whitespace, lowercasing, and ignoring empty entries. (PRD-0023 T-001)

#### Scenario: Messy list
- **WHEN** `IRONPAD_PROXY_ALLOWLIST` is ` Crates.IO , GITHUB.COM ,,`
- **THEN** the allowlist is `crates.io` and `github.com`

### Requirement: Fail-closed allowlist
The proxy MUST deny every CONNECT when `IRONPAD_PROXY_ALLOWLIST` is unset or holds no domains, and SHALL log a warning at startup that all connections will be denied. (PRD-0023 T-001)

#### Scenario: Empty allowlist
- **WHEN** the proxy runs with an empty allowlist and a client sends `CONNECT crates.io:443 HTTP/1.1`
- **THEN** the proxy replies `HTTP/1.1 403 Forbidden`

### Requirement: Domain matching
The proxy SHALL allow a CONNECT only when the target host, compared case-insensitively, equals an allowlisted domain or ends with `.` followed by one; the target port plays no part in the decision. Denied requests SHALL receive `HTTP/1.1 403 Forbidden`. (PRD-0023 T-001)

#### Scenario: Subdomain allowed
- **WHEN** the allowlist is `crates.io` and a client connects to `static.crates.io:443` or `Static.Crates.IO:443`
- **THEN** the tunnel is allowed

#### Scenario: Look-alike denied
- **WHEN** the allowlist is `crates.io` and a client connects to `evilcrates.io:443` or `evil.com:443`
- **THEN** the proxy replies `403 Forbidden`

### Requirement: Transparent tunnel
The proxy SHALL, for an allowed CONNECT, open a TCP connection to the requested `host:port`, reply `HTTP/1.1 200 Connection Established`, and relay bytes in both directions until either side closes, forwarding any bytes the client sent after the request headers; an unreachable upstream SHALL receive `HTTP/1.1 502 Bad Gateway`. (PRD-0023 T-001)

#### Scenario: Echo through the tunnel
- **WHEN** a client connects to an allowed upstream that echoes and sends `hello` after the 200 reply
- **THEN** the client reads back `hello`

#### Scenario: Pipelined payload
- **WHEN** a client sends `PIPELINED_HELLO` in the same write as the CONNECT headers
- **THEN** the upstream receives `PIPELINED_HELLO` and its echo reaches the client after the 200 reply

#### Scenario: Upstream down
- **WHEN** the requested allowed host refuses the connection
- **THEN** the proxy replies `502 Bad Gateway`

### Requirement: Bounded request parsing
The proxy SHALL reply `HTTP/1.1 413 Payload Too Large` and close the connection when the request line reaches 8 KiB without a newline or the header block reaches 64 KiB without its terminating blank line. (6e6942e)

#### Scenario: Oversized request line
- **WHEN** a client sends a `CONNECT` line with a 16 KiB host name
- **THEN** the proxy replies `413` rather than buffering it

### Requirement: Listen address
The proxy SHALL listen on `IRONPAD_PROXY_BIND` (default `127.0.0.1:3112`), exit with an error naming the address if it cannot bind, and take its log filter from `RUST_LOG` (default `info`). (PRD-0023 T-001)

#### Scenario: Default bind
- **WHEN** the proxy starts with no `IRONPAD_PROXY_BIND`
- **THEN** it accepts connections on `127.0.0.1:3112` only

### Requirement: Enabled in the deploy image
The Docker image SHALL start `ironpad-proxy` in the background before the server and set `IRONPAD_COMPILATION_PROXY=http://127.0.0.1:3112` and `IRONPAD_PROXY_ALLOWLIST=crates.io,github.com,githubusercontent.com`, so containerized instances filter cell-build egress by default. The filter is advisory: no network rule enforces it, so a build process that ignores the proxy variables reaches the network directly (PRD-0030, revised 2026-11-07). (PRD-0023 T-003 T-004)

#### Scenario: Container compile
- **WHEN** a cell with a crates.io dependency is compiled in the container
- **THEN** cargo reaches `index.crates.io` and `static.crates.io` through the proxy
- **AND** a dependency's attempt to CONNECT to any other domain through the proxy is refused with `403`
