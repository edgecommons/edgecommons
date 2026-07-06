# EdgeCommons configuration schema (single source of truth)

`edgecommons-config-schema.json` in this directory is the **canonical** JSON Schema for
EdgeCommons component configuration (the `ComponentConfig` document). All four language
libraries validate config against the **same** schema.

## Why a sync step instead of one shared file reference

Each library packages independently and embeds/loads its own copy, because every
packaging toolchain wants the file *inside* the artifact:

| Lib | Copy location | How it's loaded |
|-----|---------------|-----------------|
| Rust | `libs/rust/resources/edgecommons-config-schema.json` | `include_str!` at compile time |
| TypeScript | `libs/ts/src/config/schema.json` | `import schema from "./schema.json"` |
| Python | `libs/python/edgecommons/resources/edgecommons-config-schema.json` | package-data + `jsonschema` |
| Java | `libs/java/src/main/resources/edgecommons-config-schema.json` | classpath resource |
| (Java docs) | `libs/java/doc/edgecommons-config-schema.json` | documentation copy |

A single in-place reference would fight `cargo publish`, wheel package-data, the Maven
jar, and `tsc` rootDir. So the canonical file here is the source of truth and the copies
are generated from it.

## Editing the schema

1. Edit **`schema/edgecommons-config-schema.json`** (this file) — never edit a per-lib copy.
2. Run the sync to propagate it into every library:
   ```bash
   ./schema/sync-schema.sh          # bash / Git Bash / Linux CI
   # or
   .\schema\sync-schema.ps1         # Windows PowerShell
   ```
3. Commit the canonical file **and** the regenerated copies together.

CI (`.github/workflows/interop.yml`, job `schema-drift`) runs `sync-schema.sh --check`
and fails the build if any copy has drifted from the canonical source.

## Validation policy

- **Top level is strict** (`additionalProperties: false`) and **`component` is required**
  — unknown/mistyped top-level sections are rejected (this is what catches, e.g., a
  `parameters` section that a subsystem forgot to register).
- **Subsystem sections** whose detailed schema is owned and validated by the subsystem at
  runtime (`messaging`, `streaming`, `credentials`, `parameters`) are present as known
  sections but kept permissive (`additionalProperties: true`).
- Structural sections (`logging`, `heartbeat`, `metricEmission`, `component`, `tags`) are
  fully described and strict. Cross-language/legacy vocabulary that all libraries accept is
  reflected here (e.g. messaging `destination` accepts `ipc`/`local` and `northbound`).
