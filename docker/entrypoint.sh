#!/bin/sh
set -e

# Seed pre-built atomics sysroot into the persistent volume if missing.
# The Docker image builds these to /warmup-cache (outside /ironpad) so they
# survive the Fly persistent volume mount that shadows /ironpad at runtime.
if [ ! -d /ironpad/cache/targets/atomics-shared/wasm32-unknown-unknown ]; then
    if [ -d /warmup-cache/targets/atomics-shared ]; then
        echo "Seeding atomics sysroot into volume..."
        mkdir -p /ironpad/cache/targets
        cp -a /warmup-cache/targets/atomics-shared /ironpad/cache/targets/atomics-shared
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
    else
        echo "WARN: no pre-built default target dir in image (/warmup-cache); first compile and first live check will be cold."
    fi
fi

# Start the compilation proxy in the background, then exec the main server.
/app/ironpad-proxy &
exec /app/ironpad-server
