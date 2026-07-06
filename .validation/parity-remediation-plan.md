# EdgeCommons cross-language parity — remediation plan

Source: the evidence-based audit (memory `edgecommons-parity-gaps-audit`). This is the working
register. Designations:

- ✅ **FULL** — at parity, no action.
- 🔧 **FIX** — real gap to remediate. Priority P0 (bug) / P1 (correctness) / P2 (feature/larger).
- 🟡 **WAIVED** — known, *acceptable* deviation that will NOT be fixed; reason recorded
  (SDK = upstream SDK limitation; PLAT = platform capability; IDIOM = language-idiomatic equivalent).
- 📐 **BY-DESIGN** — intentional architectural choice; no code change (may need a doc correction).
- 📝 **DOC** — the fix is a documentation correction, not code.

Languages: J=Java, P=Python, R=Rust, T=TS.

## Deviation matrix

| # | Gap | J | P | R | T | Designation | Priority |
|---|-----|---|---|---|---|-------------|----------|
| 1 | `logging.format` honored | ✅ | ✅ | 🔧 | 🔧 | FIX (per-language format) | P2 |
| 2 | `logging.loggers` per-logger levels | ✅ | ✅ | 🔧 | 🔧 | FIX | P2 |
| 3 | `logging.globalControl` | 🟡 | 🔧 | 🔧 | 🔧 | DOC/decide (semantics unclear) | P3 |
| 4 | logging hot-reload rebuilds format/file | ✅ | ✅ | 🔧 | 🔧 | FIX (with #1) | P2 |
| 5 | config re-validated on hot-reload | 🔧 | ✅ | ✅ | ✅ | FIX | **P0** |
| 6 | validation fail-closed (no silent self-disable) | 🔧 | 🔧 | ✅ | ✅ | FIX | P1 |
| 7 | standalone request/reply reply-sub cleanup | ✅ | 🔧 | ✅ | ✅ | FIX (lib bug) | **P0** |
| 8 | standalone reconnect + resubscribe | ✅ | ✅ | ✅ | 🔧 | FIX (lib bug) | **P0** |
| 9 | standalone subscribe blocks until SUBACK | ✅ | ✅ | 🔧 | ✅ | FIX | P1 |
| 10 | IPC `receiveOwnMessages` | ✅ | ✅ | 🟡 | ✅ | **WAIVED (SDK)** — Rust GG SDK has no ReceiveMode | — |
| 11 | local-broker server-TLS (CA-only) | ✅ | ✅ | ✅ | 🔧 | FIX | P1 |
| 12 | `max_messages` queue bound | 🔧 | 🔧 | ✅ | ✅ | FIX | P2 |
| 13 | built-in request timeout | 🟡 | 🟡 | 🟡 | ✅ | WAIVED (IDIOM) — Rust tokio::timeout, J/P caller-poll | — |
| 14 | heartbeat `disk` + `fds` collected | 🔧 | ✅ | ✅ | 🟡 | FIX (Java) | P1 |
| 15 | heartbeat threads/files/fds cross-platform | ✅ | ✅ | ✅ | 🟡 | WAIVED (PLAT) — TS Linux-only (deploy target Linux) | — |
| 16 | metric `log` target rotation (`maxFileSize`) | ✅ | 🔧 | ✅ | ✅ | FIX (Python) | P1 |
| 17 | cloudwatch region from default chain | ✅ | 🔧 | ✅ | ✅ | FIX (Python) | P1 |
| 18 | heartbeat namespace from `metricEmission` | ✅ | 🔧 | ✅ | ✅ | FIX (Python) | P1 |
| 19 | heartbeat interval default 5s (no 30s fallback) | ✅ | 🔧 | ✅ | ✅ | FIX (Python) | P1 |
| 20 | dimension cap (≤10) enforced at build | 🔧 | ✅ | 🔧 | 🔧 | FIX (J partial, R/T none) | P1 |
| 21 | `Metric` direct-ctor deprecated/blocked | 🔧 | ✅ | 🟡 | ✅ | FIX (Java @Deprecated) | P3 |
| 22 | streaming native import guarded + declared dep | ✅ | 🔧 | ✅ | ✅ | FIX (Python) | P1 |
| 23 | DI / I*Service + ServiceRegistry seam | 📐 | 📝 | ✅ | ✅ | Java BY-DESIGN (removed in remediation); Python DOC (stale claim) | P2 |
| 24 | legacy `init()` facade | ✅ | 📝 | 📐 | 📐 | DOC (root CLAUDE.md overstates) | P3 |

Feature-gating (loud, not silent — track but not bugs): Rust gates GG_CONFIG/SHADOW, cloudwatch,
AWS cred/param, pkcs11 behind off-by-default cargo features. TS cloudwatch/ssm = optional npm deps.
Designation 🟡 WAIVED (BUILD) unless we decide a default build must be source-complete.

## Execution batches (by lib, to verify with one test run each)
- **Batch P0** (real bugs first): #7 Python reply-leak, #8 TS reconnect, #5 Java hot-reload re-validate.
- **Batch PY** (Python cluster): #7, #16, #17, #18, #19, #22, #6(py), #20(n/a py ok).
- **Batch TS**: #8, #11, #20(ts), #1/#2/#4(ts logging).
- **Batch RUST**: #9, #20(rust), #1/#2/#4(rust logging).
- **Batch JAVA**: #5, #6(java), #14, #20(java), #21.
- **Batch DOCS**: #23(python doc), #24, root+python CLAUDE.md DI/interfaces/init corrections; #3 decision.
- **Logging (#1/#2/#4)** handled as a focused cross-lang effort (per-language format fields in the
  shared schema + Rust/TS actually applying format + per-logger levels + reload rebuild).

Verification gate per batch: that lib's full test suite green; cross-lang interop unaffected.

## Remediation status (2026-06-22)

FIXED + verified + committed:
- #7 Python standalone reply-sub leak (880a1df, +regression test) · #16/#17/#18/#19/#22/#6 Python (880a1df)
- #8 TS standalone reconnect+resubscribe · #11 TS local TLS · #20 TS dim cap (c4f7b0a, +test)
- #5 Java re-validate on hot-reload (b8675d5, +test) · #6 Java fail-closed · #14 Java heartbeat
  disk+fds · #21 Java @Deprecated Metric ctor (45fa716)
- #9 Rust subscribe-blocks-until-SUBACK · #20 Rust dim cap (faf3033, +test, mqtt IT 5/5)
- #2 Rust per-logger levels via EnvFilter (b9fbeb2)
- #23/#24 doc corrections: root + python CLAUDE.md DI/interfaces/init claims fixed

NO CHANGE NEEDED (audit imprecision):
- #20 Java — already enforced at assembly (Metric.addDimension guard fires on category injection).

WAIVED: #10 Rust receiveOwnMessages (SDK) · #13 built-in request timeout (IDIOM) · #15 TS
heartbeat threads/files/fds on Win/mac (PLAT).

LOGGING IMPLEMENTED 2026-06-22 (commit e6310e9):
- #1 DONE — replaced logging.format with per-language java_format/python_format/rust_format/
  ts_format (schema + all libs + every in-repo config/recipe/fixture migrated). Java/Python apply
  natively; Rust renders via a custom tracing TokenLayer ({timestamp}/{level}/{target}/{message});
  TS renders a token template and re-applies it on reload.
- #2 Rust DONE (EnvFilter per-logger directives). #4 TS DONE (format+file rebuilt on reload).

#2 DONE — TS per-logger levels via getLogger(name) (hierarchical longest-prefix match on
logging.loggers, refreshed on reload; exported from the index). Rust already done via EnvFilter.
#12 DONE — per-subscription max_messages queue bound + drop-on-overflow added to Java and Python
(both transports), parity with Rust/TS; maxConcurrency unchanged; default 10000, <=0 = unbounded.
Also added rust_format/ts_format to the Rust+TS example recipes/test-configs/CLI templates.

Still DEFERRED → shared-configuration logging redesign (only logging-format-runtime items remain):
- #4 Rust format/file hot-reload rebuild — WAIVED-runtime: tracing layers can't be swapped after
  install (level + per-logger reload live); revisit if we move off the global subscriber.
- #3 logging.globalControl — semantics vestigial/inconsistent across libs; decide or remove.
