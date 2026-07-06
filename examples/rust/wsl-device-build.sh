#!/usr/bin/env bash
# Build the Rust skeleton as a Linux Greengrass device binary in WSL, into /tmp to dodge
# slow/locked /mnt builds. Includes streaming-kinesis so the durable `telemetry` + the in-memory
# `debug-trace` streams in recipe.yaml are active on-device (plus credentials + parameters).
set -euo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR=/tmp/ggrust-target
cd /mnt/c/Users/breis/source/edgecommons-monorepo/examples/rust
cargo build --release --no-default-features \
  --features greengrass,credentials,parameters,streaming-kinesis
BIN="$CARGO_TARGET_DIR/release/rust-component-skeleton"
file "$BIN"
ls -la "$BIN"
# Copy out to a Windows-accessible temp so the (git-bash) scp step can ship it to the lab.
OUT="/mnt/c/Users/breis/AppData/Local/Temp/rust-component-skeleton-device"
cp "$BIN" "$OUT"
echo "DEVICE_BINARY_READY=$OUT"
