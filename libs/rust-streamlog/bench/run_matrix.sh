#!/usr/bin/env bash
# Sweep a representative slice of the §15.3 matrix into bench/results/<target>/.
# Usage: bench/run_matrix.sh <target-name> <buffer-path> [git-sha]
# Run ON the device under test, with <buffer-path> on its real volume.
set -euo pipefail

TARGET="${1:?usage: run_matrix.sh <target-name> <buffer-path> [git-sha]}"
BUFPATH="${2:?usage: run_matrix.sh <target-name> <buffer-path> [git-sha]}"
SHA="${3:-$(git rev-parse --short HEAD 2>/dev/null || echo dev)}"

HERE="$(cd "$(dirname "$0")/.." && pwd)"   # crate root
RESULTS="$HERE/bench/results"
CORES="$(nproc 2>/dev/null || echo 4)"

run() { # run <label> -- <loadgen args...>
  local label="$1"; shift; shift
  echo "=== $label ==="
  cargo run --quiet --release --example loadgen -- \
    --path "$BUFPATH" --target-name "$TARGET" --git-sha "$SHA" --results-dir "$RESULTS" "$@"
}

# S1 — ingest throughput curve: payload × fsync, at N(cores) threads.
# Default = concurrent ingest+drain (the real running-component profile); plus an ingest-only
# ceiling per payload so the gap (lock contention + per-batch checkpoint fsync) is visible.
for payload in 256 1024 4096 65536; do
  for fsync in PerBatch Interval Always; do
    run "S1 concurrent p=$payload fsync=$fsync" -- \
      --scenario S1 --count 1000000 --payload "$payload" --threads "$CORES" \
      --fsync "$fsync" --segment-bytes 67108864 --sink instant
  done
  run "S1 ceiling p=$payload (ingest-only)" -- \
    --scenario S1 --count 1000000 --payload "$payload" --threads "$CORES" \
    --fsync PerBatch --segment-bytes 67108864 --sink ingest-only
done

# S2 — append latency at a fixed sustainable rate (single thread), concurrent drain.
run "S2 latency" -- --scenario S2 --rate 50000 --duration 20 --payload 1024 --threads 1 --sink instant

# S3 — recovery time vs log size (cold cache where possible; needs root for --drop-caches).
for n in 1000000 10000000; do
  run "S3 recovery n=$n" -- --scenario S3 --count "$n" --payload 256 --drop-caches
done

# S4 — backpressure: producer >> drain, bounded disk.
run "S4 dropOldest" -- --scenario S4 --duration 15 --payload 1024 --threads "$CORES" \
  --on-full dropOldest --max-disk-bytes 67108864 --segment-bytes 8388608 --sink fake-rate:20000
run "S4 block" -- --scenario S4 --duration 15 --payload 1024 --threads "$CORES" \
  --on-full block --max-disk-bytes 67108864 --segment-bytes 8388608 --sink fake-rate:50000

# S6 — disconnected backlog → drain.
run "S6 backlog drain" -- --scenario S6 --duration 30 --rate 50000 --payload 512 --sink disconnect:20

echo "results in $RESULTS/$TARGET/"
