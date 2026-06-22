# Contributing to GGCommons

This is a **monorepo**: the four libraries, the shared streaming core, the CLI, the templates, the
example skeletons, the canonical config schema, and the shared test infrastructure live together so
that changes which must stay consistent can land atomically.

## Layout

```
libs/{java,python,rust,ts}     # the four libraries (Java is canonical)
libs/rust-streamlog/           # ggstreamlog: shared telemetry-streaming core (+ bindings/)
cli/                           # the scaffolding CLI
templates/{java,python,rust,typescript}   # minimal component templates the CLI scaffolds from
examples/{java,python,rust,ts}            # worked example components
schema/                        # single-source config JSON schema + sync-schema.{sh,ps1}
test-infra/                    # shared EMQX broker, TLS certs, cross-language interop suite
vault-test-vectors/            # shared credentials/vault encryption conformance vectors
docs/                          # design notes
```

## The parity rule (most important)

The Java, Python, Rust, and TypeScript libraries are **deliberate mirrors** — same config schema,
same CLI contract (`-c/-m/-t`), same subsystem boundaries, and the **same MQTT message wire
format**. **Java is the canonical reference.**

When you change public behavior in one library, change the others to match in the **same PR**, and
update/extend the cross-language interop suite if the wire contract is affected. CI runs the interop
matrix on any change under `libs/**`, so divergence is caught automatically.

## The config schema is single-source

The config JSON schema lives **only** in `schema/ggcommons-config-schema.json`. After editing it,
run `schema/sync-schema.sh` (or `sync-schema.ps1`) to copy it into every library — CI fails on drift
(`schema/sync-schema.sh --check` in `.github/workflows/interop.yml`). Never hand-edit the per-library
copies.

## Running tests

```bash
# per library
( cd libs/java   && mvn -B verify )
( cd libs/python && pip install -e . && python -m pytest -m "not slow and not integration and not aws" )
( cd libs/rust   && cargo test --lib && cargo clippy --all-targets -- -D warnings )
( cd libs/ts     && npm install && npm test )
( cd cli         && pip install -e .[test] && python -m pytest )

# cross-language interop + broker-backed integration (needs Docker)
docker compose -f test-infra/compose.yaml up -d
python -m pytest test-infra/interop/test_interop.py -v
```

## Releases

Each artifact versions independently via **path-scoped tags**:
`java-lib/vX.Y.Z`, `python-lib/vX.Y.Z`, `rust-lib/vX.Y.Z`, `ts-lib/vX.Y.Z`, `cli/vX.Y.Z`, and
`streaming/vX.Y.Z` (the native streaming bindings). A release workflow filtered to that path builds
and publishes the matching package (Maven / PyPI / crates.io / npm).
