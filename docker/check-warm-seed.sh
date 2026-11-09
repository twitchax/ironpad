#!/bin/sh
# Prove the image's warm seed is one the first default cell actually reuses.
#
# Runs inside a throwaway container of the image, whose /ironpad is a fresh
# anonymous volume (`cargo make docker-uat` does
#   docker run --rm -i --entrypoint sh ironpad -s < docker/check-warm-seed.sh).
# It seeds that empty volume exactly as docker/entrypoint.sh does, scaffolds a
# cell the way the server does for a new notebook, and fails unless cargo
# reports every registry dependency fresh, for `build` and for `check`. A stale
# warmup fails nothing on its own: it only makes the first compile slow and the
# first live check time out, which is why this exists.
#
# Offline on purpose: online, cargo resolves the newest versions published
# since the image was built, which is drift a seed cannot prevent and not what
# this checks (that the warmups' CARGO_HOME, profile, edition and wasm-bindgen
# pin match a real cell's).
set -eu

if [ -n "$(ls -A /ironpad 2>/dev/null)" ]; then
    echo "check-warm-seed: /ironpad is not empty; run this in a fresh container" >&2
    exit 2
fi
mkdir -p /ironpad/cache/targets
cp -a /warmup-cache/cargo-home /ironpad/cache/cargo-home
cp -a /warmup-cache/targets/default /ironpad/cache/targets/default

cell=$(mktemp -d)
mkdir -p "$cell/src"
echo 'pub fn probe() -> u32 { 1 }' > "$cell/src/lib.rs"
# What compiler/scaffold.rs `generate_cargo_toml` writes for a cell of a new
# notebook, whose shared Cargo.toml is DEFAULT_SHARED_CARGO_TOML.
# `warmup_manifests_match_the_default_notebook_profile` (compiler/build.rs)
# holds the profile lines and edition here to that constant.
cat > "$cell/Cargo.toml" <<MANIFEST
[package]
name = "cell-warm-seed-check"
version = "0.1.0"
edition = "2024"

[lib]
crate-type = ["cdylib"]

[workspace]

[dependencies]
ironpad-cell = { path = "/app/crates/ironpad-cell" }
wasm-bindgen = "=$(wasm-bindgen --version | cut -d' ' -f2)"

[profile.release]
opt-level = 1
lto = false
codegen-units = 16
MANIFEST

cd "$cell"
status=0
for sub in build check; do
    # The image's default toolchain is CELL_TOOLCHAIN (asserted by
    # toolchain_pins_are_in_sync_across_dockerfile_ci_and_toolchain_toml).
    CARGO_HOME=/ironpad/cache/cargo-home CARGO_TARGET_DIR=/ironpad/cache/targets/default \
        cargo "$sub" --offline --target wasm32-unknown-unknown --release \
        --message-format=json > "$sub.json"
    artifacts=$(grep '"reason":"compiler-artifact"' "$sub.json" | grep '"package_id":"registry+' || true)
    total=$(printf '%s' "$artifacts" | grep -c . || true)
    stale=$(printf '%s' "$artifacts" | grep '"fresh":false' \
        | sed -E 's/.*"package_id":"registry\+[^#]*#([^"]*)".*/\1/' || true)
    if [ "$total" -eq 0 ]; then
        echo "check-warm-seed: $sub reported no registry artifacts at all" >&2
        status=1
    elif [ -n "$stale" ]; then
        echo "check-warm-seed: $sub rebuilt $(printf '%s\n' "$stale" | grep -c .) of $total registry crates the seed should have covered:" >&2
        printf '  %s\n' $stale >&2
        status=1
    else
        echo "check-warm-seed: $sub reused all $total registry crates from the seed"
    fi
done
exit "$status"
