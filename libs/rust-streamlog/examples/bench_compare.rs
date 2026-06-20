//! `bench_compare` — flag perf regressions between two `loadgen` result JSON files (spec §15.8).
//!
//! No absolute pass/fail: compares a current run against a recorded baseline for the **same
//! target/config** and flags any metric that regresses by more than the threshold (default 10%).
//! Exits non-zero if any regression is found (CI gate).
//!
//! ```text
//! cargo run --release --example bench_compare -- \
//!   --baseline bench/results/lab-5950x/S1-<old-sha>.json \
//!   --current  bench/results/lab-5950x/S1-<new-sha>.json [--threshold 0.10]
//! ```

use std::collections::HashMap;

use serde_json::Value;

/// Whether a higher value is *worse* (a regression) for a given metric.
fn higher_is_worse(metric: &str) -> bool {
    matches!(
        metric,
        "append_p50_ns"
            | "append_p99_ns"
            | "append_p999_ns"
            | "append_max_ns"
            | "recovery_ms"
            | "rss_peak_bytes"
            | "export_lag_p99_ms"
            | "time_to_catchup_ms"
            | "dropped_total"
    )
}

/// Metrics worth comparing (others are descriptive, not perf signals).
const COMPARED: &[&str] = &[
    "throughput_rps",
    "throughput_mb_s",
    "logical_write_mb_s",
    "append_p50_ns",
    "append_p99_ns",
    "append_p999_ns",
    // append_max_ns is deliberately NOT gated — a single outlier makes it too noisy for CI.
    "recovery_ms",
    "drain_rate_rps",
    "time_to_catchup_ms",
    "export_lag_p99_ms",
    "rss_peak_bytes",
];

fn args() -> HashMap<String, String> {
    let mut m = HashMap::new();
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        if let Some(k) = a.strip_prefix("--") {
            if let Some(v) = it.next() {
                m.insert(k.to_string(), v);
            }
        }
    }
    m
}

fn load(path: &str) -> Value {
    let s = std::fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("bench_compare: cannot read {path}: {e}");
        std::process::exit(2);
    });
    serde_json::from_str(&s).unwrap_or_else(|e| {
        eprintln!("bench_compare: invalid JSON in {path}: {e}");
        std::process::exit(2);
    })
}

fn num(v: &Value, key: &str) -> Option<f64> {
    v.get("metrics")?.get(key)?.as_f64()
}

/// The config block minus volatile fields (the buffer `path` differs between runs by design).
fn config_signature(v: &Value) -> Value {
    let mut cfg = v.get("config").cloned().unwrap_or(Value::Null);
    if let Some(obj) = cfg.as_object_mut() {
        obj.remove("path");
    }
    cfg
}

fn main() {
    let a = args();
    let baseline_path = a.get("baseline").cloned().unwrap_or_else(|| {
        eprintln!("bench_compare: --baseline <file> is required");
        std::process::exit(2);
    });
    let current_path = a.get("current").cloned().unwrap_or_else(|| {
        eprintln!("bench_compare: --current <file> is required");
        std::process::exit(2);
    });
    let threshold: f64 = a.get("threshold").and_then(|s| s.parse().ok()).unwrap_or(0.10);

    let base = load(&baseline_path);
    let cur = load(&current_path);

    // Sanity: same target + scenario (a different config makes the comparison meaningless).
    let bt = base.get("target_name").and_then(Value::as_str).unwrap_or("?");
    let ct = cur.get("target_name").and_then(Value::as_str).unwrap_or("?");
    let bs = base.get("scenario").and_then(Value::as_str).unwrap_or("?");
    let cs = cur.get("scenario").and_then(Value::as_str).unwrap_or("?");
    if bt != ct || bs != cs {
        eprintln!("bench_compare: WARNING comparing different runs (target {bt} vs {ct}, scenario {bs} vs {cs})");
    }
    if config_signature(&base) != config_signature(&cur) {
        eprintln!("bench_compare: WARNING config blocks differ (ignoring path) — regression flags may be noise");
    }

    println!(
        "comparing {bs}@{bt}: baseline {} vs current {}  (threshold {:.0}%)",
        base.get("git_sha").and_then(Value::as_str).unwrap_or("?"),
        cur.get("git_sha").and_then(Value::as_str).unwrap_or("?"),
        threshold * 100.0
    );
    println!("{:<22} {:>14} {:>14} {:>9}  status", "metric", "baseline", "current", "delta");

    let mut regressions = 0;
    for &metric in COMPARED {
        let (Some(b), Some(c)) = (num(&base, metric), num(&cur, metric)) else {
            continue;
        };
        if b == 0.0 {
            continue;
        }
        let worse_up = higher_is_worse(metric);
        // Signed delta where positive = improvement.
        let pct = if worse_up { (b - c) / b } else { (c - b) / b };
        let regressed = pct < -threshold;
        let status = if regressed {
            regressions += 1;
            "REGRESSION"
        } else if pct > threshold {
            "improved"
        } else {
            "ok"
        };
        println!("{metric:<22} {b:>14.2} {c:>14.2} {:>8.1}%  {status}", pct * 100.0);
    }

    if regressions > 0 {
        eprintln!("\nbench_compare: {regressions} metric(s) regressed > {:.0}%", threshold * 100.0);
        std::process::exit(1);
    }
    println!("\nbench_compare: no regressions > {:.0}%", threshold * 100.0);
}
