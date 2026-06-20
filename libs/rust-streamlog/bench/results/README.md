# Baseline results

`loadgen --results-dir bench/results` writes here as `<target>/<scenario>-<git-sha>.json`, e.g.
`lab-5950x/S1-ab12cd3.json`. Commit one baseline per **target × scenario × config** and compare new
runs against it with `examples/bench_compare.rs` (see `../README.md` §15.8).

Targets: `windows-dev` (durability path + x86_64 upper bound, not a deployment target),
`lab-5950x` (primary Linux perf + real Kinesis), `pi5-sd` / `pi5-ssd` (constrained-edge floor vs
range). Treat Pi-on-microSD as the conservative edge floor and 5950X/NVMe as the high end.
