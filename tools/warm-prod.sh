#!/usr/bin/env bash
# Converge a freshly deployed (or cache-cleared) ironpad instance's compile and
# check caches so first visitors don't eat cold-build latency and live checks
# don't time out (the 10s production check budget assumes warm artifacts).
#
# Technique (see the agents guide, "Browser-Cache Hygiene" / deploy notes):
# server fns accept application/x-www-form-urlencoded bodies with
# `request[field]=...` encoding. Endpoint URLs carry a build-specific hash, so
# they are discovered from the deployed wasm binary rather than hardcoded.
# Killed/timed-out builds retain completed compilation units, so repeated
# rounds converge: TimedOut -> TimedOut -> Clean is normal on a cold volume.
#
# Usage: tools/warm-prod.sh [base-url]        (default: the Fly deployment)
#   WARM_MAX_ROUNDS (default 20) and WARM_SLEEP_SECS (default 5) tune the loop.
#
# Scope: warms the default-deps (plain) and simd lanes. autodiff and rayon
# cells rebuild for minutes on the 2-CPU box; warm those by loading their
# public notebooks once and letting the loop below carry the retries.

set -euo pipefail

BASE="${1:-https://twitchax-ironpad.fly.dev}"
MAX_ROUNDS="${WARM_MAX_ROUNDS:-20}"
SLEEP_SECS="${WARM_SLEEP_SECS:-5}"

# ── Endpoint discovery ────────────────────────────────────────────────────────

wasm_path=$(curl -sf "$BASE/" | grep -oE '/pkg/ironpad[^"]*\.wasm' | head -1)
if [[ -z "$wasm_path" ]]; then
  echo "error: could not find a /pkg/ironpad*.wasm reference at $BASE/" >&2
  exit 1
fi

endpoints=$(curl -sf "$BASE$wasm_path" | strings | grep -oE 'api/(check_cell|compile_cell)[0-9]*' | sort -u)
compile_ep=$(grep 'compile_cell' <<<"$endpoints" | head -1)
check_ep=$(grep 'check_cell' <<<"$endpoints" | head -1)
if [[ -z "$compile_ep" || -z "$check_ep" ]]; then
  echo "error: could not extract server-fn endpoints from $wasm_path" >&2
  exit 1
fi
echo "endpoints: $compile_ep, $check_ep (from $wasm_path)"

# ── Warm bodies ───────────────────────────────────────────────────────────────

# A trivial default-deps cell: exercises the shared build/check target dir.
plain_body="request[notebook_id]=warmup&request[cell_id]=warm_plain&request[source]=let x = 41 %2B 1; format!(\"warm {x}\")&request[cargo_toml]=[dependencies]"

# A minimal simd cell: exercises the +simd128 codegen lane (no std rebuild).
simd_source='use std::simd::f32x4; let v = f32x4::splat(2.0) * f32x4::splat(3.0); format!("warm {:?}", v.to_array())'
simd_body="request[notebook_id]=warmup&request[cell_id]=warm_simd&request[source]=$(python3 -c 'import sys, urllib.parse; print(urllib.parse.quote(sys.argv[1]))' "$simd_source")&request[cargo_toml]=[dependencies]"

# A minimal Linux cell (PRD-0066): a whole program for
# wasm32-browserpod-linux-musl, which has its own toolchain, its own target dir
# and therefore its own cold build. Warming it costs NO BrowserPod allowance:
# compilation is server-side and a boot is the only metered event, so this
# converges the one lane a reader of /public/linux-cells would otherwise pay
# for on the first click.
linux_source='fn main() { println!("warm"); }'
linux_body="request[notebook_id]=warmup&request[cell_id]=warm_linux&request[source]=$(python3 -c 'import sys, urllib.parse; print(urllib.parse.quote(sys.argv[1]))' "$linux_source")&request[cargo_toml]=[dependencies]&request[cell_type]=Linux"

# ── Convergence loops ─────────────────────────────────────────────────────────

# warm_compile <label> <body>
# A successful compile returns HTTP 200 with the (large) wasm blob; the body is
# discarded and the status code is the signal. Cold builds may 500 on timeout;
# killed builds keep their completed units, so rounds converge.
warm_compile() {
  local label="$1" body="$2" code
  for ((round = 1; round <= MAX_ROUNDS; round++)); do
    code=$(curl -s -m 600 -o /dev/null -w '%{http_code}' -X POST "$BASE/$compile_ep" \
      -H 'Content-Type: application/x-www-form-urlencoded' \
      --data "$body" || echo "000")
    echo "  $label round $round: HTTP $code"
    if [[ "$code" == "200" ]]; then
      echo "  $label: converged in $round round(s)"
      return 0
    fi
    sleep "$SLEEP_SECS"
  done
  echo "  $label: NOT converged after $MAX_ROUNDS rounds" >&2
  return 1
}

# warm_check <label> <body>
# Rounds until the check reports Clean (TimedOut is the expected cold answer;
# Errors would mean the warm cell's source is broken and is a hard failure).
warm_check() {
  local label="$1" body="$2" resp
  for ((round = 1; round <= MAX_ROUNDS; round++)); do
    resp=$(curl -s -m 600 -X POST "$BASE/$check_ep" \
      -H 'Content-Type: application/x-www-form-urlencoded' \
      --data "$body" || echo '{"transport":"error"}')
    echo "  $label round $round: ${resp:0:120}"
    if grep -q '"status":"Clean"' <<<"$resp"; then
      echo "  $label: converged in $round round(s)"
      return 0
    fi
    if grep -q '"status":"Errors"' <<<"$resp"; then
      echo "  $label: warm cell has compile errors; fix tools/warm-prod.sh" >&2
      return 1
    fi
    sleep "$SLEEP_SECS"
  done
  echo "  $label: NOT converged after $MAX_ROUNDS rounds" >&2
  return 1
}

status=0
echo "warming compile (plain)..."
warm_compile "compile/plain" "$plain_body" || status=1
echo "warming check (plain)..."
warm_check "check/plain" "$plain_body" || status=1
echo "warming compile (simd)..."
warm_compile "compile/simd" "$simd_body" || status=1
echo "warming check (simd)..."
warm_check "check/simd" "$simd_body" || status=1
# Compile only: a reader of a Linux notebook never types into it, so the
# check lane is not on their path.
echo "warming compile (linux)..."
warm_compile "compile/linux" "$linux_body" || status=1

exit "$status"
