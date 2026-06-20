# Sweep a representative slice of the §15.3 matrix into bench/results/<target>/ (Windows).
# Usage: bench\run_matrix.ps1 <target-name> <buffer-path> [git-sha]
# The Windows box validates the durability path + an x86_64 upper bound, not a deployment target.
param(
  [Parameter(Mandatory = $true)][string]$Target,
  [Parameter(Mandatory = $true)][string]$BufPath,
  [string]$Sha = $(try { git rev-parse --short HEAD } catch { "dev" })
)
$ErrorActionPreference = "Stop"
$Here = Split-Path -Parent (Split-Path -Parent $PSCommandPath)   # crate root
$Results = Join-Path $Here "bench\results"
$Cores = [Environment]::ProcessorCount

function Run([string]$Label, [string[]]$Args) {
  Write-Host "=== $Label ==="
  & cargo run --quiet --release --example loadgen -- `
    --path $BufPath --target-name $Target --git-sha $Sha --results-dir $Results @Args
}

# S1 — ingest throughput curve: payload × fsync, at N(cores) threads, pure ingest.
foreach ($payload in 256, 1024, 4096, 65536) {
  foreach ($fsync in "PerBatch", "Interval", "Always") {
    Run "S1 p=$payload fsync=$fsync" @(
      "--scenario", "S1", "--count", "1000000", "--payload", "$payload",
      "--threads", "$Cores", "--fsync", "$fsync", "--segment-bytes", "67108864", "--sink", "none")
  }
}

# S2 — append latency at a fixed sustainable rate (single thread).
Run "S2 latency" @("--scenario", "S2", "--rate", "50000", "--duration", "20",
  "--payload", "1024", "--threads", "1", "--sink", "none")

# S3 — recovery time vs log size (warm cache on Windows; drop_caches is Linux-only).
foreach ($n in 1000000, 10000000) {
  Run "S3 recovery n=$n" @("--scenario", "S3", "--count", "$n", "--payload", "256")
}

# S4 — backpressure.
Run "S4 dropOldest" @("--scenario", "S4", "--duration", "15", "--payload", "1024",
  "--threads", "$Cores", "--on-full", "dropOldest", "--max-disk-bytes", "67108864",
  "--segment-bytes", "8388608", "--sink", "fake-rate:20000")

# S6 — disconnected backlog -> drain.
Run "S6 backlog drain" @("--scenario", "S6", "--duration", "30", "--rate", "50000",
  "--payload", "512", "--sink", "disconnect:20")

Write-Host "results in $Results\$Target\"
