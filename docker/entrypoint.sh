#!/bin/sh
set -e

# Seed pre-built atomics sysroot into the persistent volume if missing.
# The Docker image builds these to /warmup-cache (outside /ironpad) so they
# survive the Fly persistent volume mount that shadows /ironpad at runtime.
if [ ! -d /ironpad/cache/targets/atomics-shared/wasm32-unknown-unknown ]; then
    echo "Seeding atomics sysroot into volume..."
    mkdir -p /ironpad/cache/targets
    cp -a /warmup-cache/targets/atomics-shared /ironpad/cache/targets/atomics-shared
fi

# Start the compilation proxy in the background, then exec the main server.
/app/ironpad-proxy &
exec /app/ironpad-server
