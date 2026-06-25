# Delivering & building the ggcommons native core (`ggstreamlog`)

The telemetry-streaming engine (`ggstreamlog`) and everything built on it — `gg.streams()` and the
**durable CloudWatch metrics buffer** (the default for the `cloudwatch` target) — are backed by a
compiled Rust core. That core is **not** part of the pure-language package source; it's a
platform-specific native artifact that each language loads at runtime:

| Language | How it loads the core | Failure if absent |
|----------|-----------------------|-------------------|
| **Java** | A cdylib bundled in the jar at `/native/<os>-<arch>/`, located via `-Dggstreamlog.library.path`, `java.library.path`, or extracted from the jar. Needs FFM (`--enable-native-access=ALL-UNNAMED`, Java 22+). | `IllegalStateException` from `GgStreamNative` |
| **Python** | `import ggstreamlog_native` — a maturin-built wheel (PyPI `ggstreamlog-native`). | `ImportError` → `GgStreamError` |
| **Node** | `require("ggstreamlog-node")` — a napi-rs addon (lazy; importing the JS module does **not** load it). | `Error` from `require` |
| **Rust** | Compiled in, gated by cargo features (`streaming`, `metrics-cloudwatch-durable`, …). | feature simply not present |

This doc describes **what we prebuild** (the batteries-included happy path) and **how to build the
core yourself** for any target we don't ship — a different platform/libc, a smaller feature set, or
no streaming at all.

---

## 1. The prebuilt set (what ships)

We publish **all-features** prebuilt natives for four targets — two production Linux targets and two
developer-machine targets. (Apple-Silicon only for macOS; Intel macs are out of scope.)

| Platform | Rust target triple | Java dir (`/native/…`) | Python wheel | Node target |
|----------|--------------------|------------------------|--------------|-------------|
| Linux x86_64 (glibc) | `x86_64-unknown-linux-gnu` | `linux-x86_64` | `manylinux_2_28_x86_64` | `x86_64-unknown-linux-gnu` |
| Linux aarch64 (glibc) | `aarch64-unknown-linux-gnu` | `linux-aarch64` | `manylinux_2_28_aarch64` | `aarch64-unknown-linux-gnu` |
| Windows x64 | `x86_64-pc-windows-msvc` | `windows-x86_64` | `win_amd64` | `x86_64-pc-windows-msvc` |
| macOS arm64 | `aarch64-apple-darwin` | `darwin-aarch64` | `macosx_11_0_arm64` | `aarch64-apple-darwin` |

How each language consumes the set:

- **Java** — *one* multi-arch fat jar carries all four `/native/<os>-<arch>/` dirs; the FFM loader
  picks the right one at runtime. (The platform tags above are exactly what `GgStreamNative.osArch()`
  computes: `os ∈ {linux,windows,darwin}`, `arch ∈ {x86_64,aarch64}`.)
- **Python** — four wheels on PyPI; `pip` auto-selects by platform tag. Built `abi3-py39`, so **one
  wheel per platform** covers Python 3.9+.
- **Node** — four napi prebuilds; the generated `index.js` auto-selects by platform **and libc**.
- **Rust** — nothing to prebuild: `cargo` compiles the core in. You only choose features (§3).

### Feature scope of the prebuilts

| Artifact | Built with | Includes |
|----------|-----------|----------|
| Java cdylib | `cabi,kinesis,kafka` | durable buffer + callback sink, Kinesis sink, Kafka sink |
| Python wheel | `kinesis,kafka` | durable buffer + callback sink, Kinesis sink, Kafka sink |
| Node addon | `kinesis,kafka` | durable buffer + callback sink, Kinesis sink, Kafka sink |

> The Python/Node bindings expose `kinesis` and `kafka` passthrough features
> (`libs/rust-streamlog/bindings/{python,node}/Cargo.toml`); the release CI builds them
> `--features kinesis,kafka`. Kafka builds librdkafka via cmake (vendored, no system librdkafka at
> runtime), so the build host needs `cmake` + a C compiler — see §3.B.
>
> **Per-platform exception:** the **aarch64-linux** prebuilt ships `kinesis` only (durable buffer +
> Kinesis, no Kafka). Cross-compiling `librdkafka` (a large C library) to arm64 is impractical
> (slow under emulation, fragile to true-cross), so Kafka on arm64-linux is a **source build** (§3).
> x86_64-linux, Windows, and macOS-arm64 ship the full kinesis+kafka set.

---

## 2. Feature → use-case map (the key to footprint)

You rarely need "all features." The core's weight comes almost entirely from the AWS SDK (`kinesis`)
and librdkafka (`kafka`). The **durable CloudWatch metrics buffer uses the in-core callback sink**
(`cabi`), *not* the Kinesis/Kafka sinks — so it needs none of the heavy deps:

| You want… | cdylib features (Java) | Python/Node binding | Pulls in |
|-----------|------------------------|---------------------|----------|
| Durable CloudWatch metrics only | `cabi` | base wheel/addon (no `kinesis`) | nothing heavy |
| `gg.streams()` → Kinesis | `cabi,kinesis` | `--features kinesis` | AWS SDK |
| `gg.streams()` → Kafka | `cabi,kafka` | `--features kafka` | librdkafka (cmake) |
| Everything | `cabi,kinesis,kafka` | `--features kinesis,kafka` | AWS SDK + librdkafka |
| No streaming/durable at all | *(don't build/bundle)* | *(don't install)* | nothing |

---

## 3. Building the core for another target

### A. A different platform / libc (e.g. musl / Alpine)

The prebuilts are **glibc** (and MSVC/Apple). For musl/Alpine you must build a separate set.

```bash
# Rust target (example: arm64 Alpine)
rustup target add aarch64-unknown-linux-musl

# Java cdylib
cd libs/rust-streamlog
cargo build --release --features cabi,kinesis --target aarch64-unknown-linux-musl
#   -> target/aarch64-unknown-linux-musl/release/libggstreamlog.so

# Python wheel (musllinux tag so pip on Alpine selects it)
pip install maturin
cd libs/rust-streamlog/bindings/python
maturin build --release --features kinesis \
  --target aarch64-unknown-linux-musl --manylinux musllinux_1_2

# Node addon (napi names it ...-linux-arm64-musl; index.js loads it on Alpine automatically)
cd libs/rust-streamlog/bindings/node && npm install
npx napi build --platform --release --target aarch64-unknown-linux-musl --features kinesis
```

**musl caveats** (these bite in production, not just at build time):

- **Java can't auto-distinguish glibc vs musl** — `osArch()` yields `linux-aarch64` for *both*. So a
  jar can carry only one `linux-aarch64` cdylib. **Build a separate musl jar** (don't mix a musl `.so`
  into the glibc jar), and run it on a **musl JDK** (e.g. `eclipse-temurin:*-alpine`). Python (`pip`
  via wheel tags) and Node (napi libc detection) *do* select the right variant automatically.
- **Tiny default thread stacks** (~128 KB vs glibc's ~8 MB) — set a larger stack for the export
  threads or export `RUST_MIN_STACK` to avoid overflows.
- **DNS/NSS differences** — no LDAP/SSSD/mDNS via NSS; older musl lacked DNS TCP fallback for large
  responses. Validate cloud-endpoint resolution on the target.

> If small images aren't a hard requirement, a glibc base (`debian-slim`/`distroless`) avoids all of
> the above and reuses the prebuilt artifacts directly.

### B. A smaller feature set

Drop the sinks you don't use (see the map in §2). `kinesis` removes the AWS SDK; `kafka` removes the
librdkafka cmake build.

```bash
cd libs/rust-streamlog
# Durable-CloudWatch-only, no AWS SDK, no Kafka — smallest core that supports the default metrics path:
cargo build --release --features cabi
# Add Kinesis for gg.streams() → Kinesis:
cargo build --release --features cabi,kinesis
```

- **Kafka build prerequisites**: `cabi,kafka` (or `…,kafka`) builds **librdkafka via cmake**, so the
  build host needs **`cmake` + a C compiler** (vendored, no system librdkafka at runtime). The default
  build is plaintext; **SASL_SSL needs OpenSSL** wired into the `rdkafka` build — add that only if you
  export to a secured Kafka.

### C. Omit streaming entirely (slim build)

If a component uses only config/messaging/metrics-to-non-cloudwatch and never touches `gg.streams()`
or the durable CloudWatch buffer:

- **Java/Python/Node** — don't bundle/install the native; the lazy loader never loads it. Set the
  cloudwatch target to `targetConfig.cloudwatch.buffer: { "type": "memory" }` so it doesn't try the
  durable path (or just don't use the cloudwatch target). Nothing native is required.
- **Rust** — leave the `streaming` / `metrics-cloudwatch-durable` features off (the default).

---

## 4. If the core is missing at runtime — fail-fast

The durable CloudWatch buffer is the **default** for the `cloudwatch` target, and the native core is
bundled by design (§1). So the libraries **fail fast on an absent core**: when the durable buffer is
selected (the default, or an explicit `buffer.type: durable`) and the native core can't be loaded,
the `cloudwatch` target raises at startup instead of silently degrading — silent degradation would
lose the disconnect-tolerance you rely on. The error names the missing core and points here, so an
operator on an unshipped target (musl, exotic arch) knows to build per §3 (or set
`buffer.type: memory`) rather than discover the gap through missing metrics.

Behavior matrix when the durable buffer is selected:

| Condition | Java / Python | TypeScript / Rust |
|-----------|---------------|-------------------|
| native core absent / not loadable | **fail fast** (clear, actionable error) | **fail fast** |
| core present, buffer can't open (e.g. unwritable path) | soft fallback to in-memory (`WARN`) | **fail fast** |
| `buffer.type: memory` | in-memory (no core needed) | in-memory (no core needed) |

One intentional asymmetry: Java and Python degrade to in-memory if the core is present but the buffer
path can't be opened (a recoverable ops error), whereas the greenfield TypeScript and Rust targets
are strict and fail on any durable-init failure. Rust durability is additionally a compile-time
feature — enable `metrics-cloudwatch-durable`; an open failure then propagates as an error.

---

## 5. CI: how the prebuilts are produced

The prebuilt set is built and published by the `streaming/v*` path-scoped tag in
[`.github/workflows/release.yml`](../.github/workflows/release.yml):

```bash
git tag streaming/v0.1.0 && git push origin streaming/v0.1.0
```

Three jobs fan out across the four-target matrix:

- `streaming-python` — `PyO3/maturin-action` builds one abi3 wheel per target (manylinux_2_28
  container for the Linux targets), publishes to PyPI when `MATURIN_PYPI_TOKEN` is set.
- `streaming-node` — `napi build --target … --features kinesis` per target, publishes per-platform
  npm packages when `NPM_TOKEN` is set.
- `streaming-java-natives` + `streaming-java-jar` — build the `cabi,kinesis,kafka` cdylib on each
  runner, stage it under `native/<os>-<arch>/` (tags matching `osArch()`), then a final job assembles
  **one** fat jar carrying all four and deploys it when `MAVEN_DEPLOY_TOKEN` is set.

Publish steps are gated on the matching registry secret, so a tag push (or a manual
`workflow_dispatch`) without credentials still **builds + uploads the artifacts to the run** for
inspection.

> **aarch64-Linux is true-cross-compiled — no arm64 runner needed.** To keep the repo private on a
> free plan, the aarch64-linux artifacts are produced on x86_64 `ubuntu-latest`: `PyO3/maturin-action`
> cross-builds the wheel, and the node addon + Java cdylib use `cross-rs` (`--cross-compile` /
> `cross build`, configured by `Cross.toml`, which installs cmake + Go for the AWS-SDK crypto
> cross-build). aarch64-linux ships `kinesis` only — dropping Kafka removes `librdkafka`, the one
> dependency that made arm64 cross impractical. (If you later make the repo public or move to a
> Team/Enterprise plan, you can add a native `ubuntu-24.04-arm` runner to also ship Kafka on arm64.)
