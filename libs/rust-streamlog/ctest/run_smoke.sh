#!/usr/bin/env bash
# Build the edgestreamlog cdylib (feature `cabi`) and run the C smoke test against it.
# Linux/WSL (gcc + .so). Optionally pass `kinesis` as $1 to also build the AWS sink in.
set -euo pipefail

HERE="$(cd "$(dirname "$0")/.." && pwd)"   # crate root
CARGO="${CARGO:-$HOME/.cargo/bin/cargo}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/tmp/esl-cabi-target}"

FEATURES="cabi"
[ "${1:-}" = "kinesis" ] && FEATURES="cabi,kinesis"

echo "building cdylib (features: $FEATURES) ..."
"$CARGO" build --manifest-path "$HERE/Cargo.toml" --features "$FEATURES" --release

LIBDIR="$CARGO_TARGET_DIR/release"
echo "compiling + running C smoke test ..."
gcc -std=c11 -Wall -Wextra -Wpedantic -I"$HERE/include" "$HERE/ctest/smoke.c" \
    -L"$LIBDIR" -ledgestreamlog -Wl,-rpath,"$LIBDIR" -o /tmp/esl_smoke
rm -rf /tmp/esl-smoke
/tmp/esl_smoke
