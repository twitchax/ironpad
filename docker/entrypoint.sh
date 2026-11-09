#!/bin/sh
set -e

# Set when a seed below copies root-owned files into the build cache, so the
# hand-over to the build user further down runs again.
seeded=0

# Seed pre-built atomics sysroot into the persistent volume if missing.
# The Docker image builds these to /warmup-cache (outside /ironpad) so they
# survive the Fly persistent volume mount that shadows /ironpad at runtime.
if [ ! -d /ironpad/cache/targets/atomics-shared/wasm32-unknown-unknown ]; then
    if [ -d /warmup-cache/targets/atomics-shared ]; then
        echo "Seeding atomics sysroot into volume..."
        mkdir -p /ironpad/cache/targets
        cp -a /warmup-cache/targets/atomics-shared /ironpad/cache/targets/atomics-shared
        seeded=1
    else
        # Never crash-loop the whole server over a missing optimization: the
        # first rayon cell just builds std cold instead (slow, not fatal).
        echo "WARN: no pre-built atomics sysroot in image (/warmup-cache); first rayon cell will build std cold."
    fi
fi

# Seed the pre-built default cell target dir (build + check artifacts for
# the ironpad-cell tree) so first compiles are fast and live check-on-type
# is warm from boot (PRD-0045). Same pattern and same graceful degradation
# as the atomics seed above.
if [ ! -d /ironpad/cache/targets/default/wasm32-unknown-unknown ]; then
    if [ -d /warmup-cache/targets/default ]; then
        echo "Seeding default cell target dir into volume..."
        mkdir -p /ironpad/cache/targets
        cp -a /warmup-cache/targets/default /ironpad/cache/targets/default
        seeded=1
    else
        echo "WARN: no pre-built default target dir in image (/warmup-cache); first compile and first live check will be cold."
    fi
fi

# ── Cell builds run as the unprivileged build user ──────────────────────────
#
# The server stays root; every cargo process it starts runs as BUILD_UID
# (crates/ironpad-app/src/compiler/build_user.rs). That user owns what cargo
# writes, the target dirs, CARGO_HOME and its own HOME, and nothing else: the
# accounts database, private notebooks and the served blob cache are closed to
# it below.
#
# Handing a tree over walks every file in it (targets alone runs to GBs, and
# cold start is a measured concern), so it happens once per volume, recorded
# by a marker, and again only when a seed has just copied root-owned files in.
# Afterwards the server hands over the top level of any directory it creates
# itself, and everything cargo creates inside one is the build user's already.
# `-h` changes a symlink rather than its target, and -R does not follow links,
# so a link a build left in the cache cannot aim this chown elsewhere.
BUILD_UID=2000
if [ "$seeded" = 1 ] || [ "$(cat /ironpad/cache/.build-owner 2>/dev/null)" != "$BUILD_UID" ]; then
    echo "Handing the build cache to uid $BUILD_UID..."
    mkdir -p /ironpad/cache/targets /ironpad/cache/cargo-home /ironpad/cache/build-home
    chown -R -h "$BUILD_UID:$BUILD_UID" \
        /ironpad/cache/targets /ironpad/cache/cargo-home /ironpad/cache/build-home
    echo "$BUILD_UID" > /ironpad/cache/.build-owner
fi
mkdir -p /ironpad/data /ironpad/cache/blobs
chmod 700 /ironpad/data /ironpad/cache/blobs
export IRONPAD_BUILD_UID="$BUILD_UID"

# Start the compilation proxy in the background, then exec the main server.
/app/ironpad-proxy &
exec /app/ironpad-server
