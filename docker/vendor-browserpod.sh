#!/usr/bin/env bash
# Fetch and verify the BrowserPod Rust toolchain tarball into docker/vendor/
# (PRD-0066 T-012).
#
# The image build installs the toolchain from this vendored copy rather than
# piping the vendor's install.sh to a shell, so a 20-minute production build
# cannot fail on somebody else's CDN. What is left is this script: a single
# cheap, retryable, cacheable step that runs BEFORE the build and can be
# pointed at a mirror by setting BROWSERPOD_DIST_BASE.
#
# Idempotent: an existing file with the right digest is kept, so repeated
# `cargo make docker-build` runs re-download nothing.
set -euo pipefail

cd "$(dirname "$0")/.."
# shellcheck source=docker/browserpod.env
. docker/browserpod.env

tarball="browserpod-rust-${BROWSERPOD_VERSION}.tar.gz"
dest="docker/vendor/${tarball}"

verify() {
    echo "${BROWSERPOD_SHA256}  $1" | sha256sum -c - >/dev/null 2>&1
}

if [ -f "$dest" ] && verify "$dest"; then
    echo "==> $dest already vendored and verified."
    exit 0
fi

mkdir -p docker/vendor
echo "==> Fetching ${BROWSERPOD_DIST_BASE}/${tarball} ..."
# To a temp name first: a half-written file that happens to survive would fail
# verification below, but leaving it in place means the next run has to notice
# and delete it. Nothing lands at $dest unless it is whole and correct.
tmp="${dest}.partial"
trap 'rm -f "$tmp"' EXIT
curl -fSL --proto '=https' --tlsv1.2 "${BROWSERPOD_DIST_BASE}/${tarball}" -o "$tmp"

if ! verify "$tmp"; then
    echo "ERROR: sha256 mismatch for ${tarball}" >&2
    echo "  expected: ${BROWSERPOD_SHA256}" >&2
    echo "  actual:   $(sha256sum "$tmp" | cut -d' ' -f1)" >&2
    exit 1
fi

mv "$tmp" "$dest"
trap - EXIT
echo "==> Vendored $dest ($(du -h "$dest" | cut -f1), sha256 verified)."
