# Contributing a component to the registry

1. **Scaffold** your component with the CLI (`edgecommons create-component -n <name> -l <LANG>`) and
   push it to a repo under the `edgecommons` org (or your own, if external).
2. **Name** the repo with a flat, lowercase, hyphenated name describing what it does
   (`opcua-adapter`, `s7-adapter`, `rollup-processor`, `kafka-sink`). Do **not** prefix with
   `edgecommons-` — the org namespaces it.
3. **Topic** the repo: `edgecommons`, the category topic (`edgecommons-adapter` /
   `edgecommons-processor` / `edgecommons-sink`), plus `aws-iot-greengrass`, `iiot`, and a protocol
   topic where relevant.
4. **Add an entry** to `components.json` and open a PR. Required fields: `name`, `repo`, `language`,
   `category`, `description`. Recommended: `protocol`, `status`, `platforms`, `library`, `topics`.
   See `registry.schema.json` for the full contract.

## Categories

| Category | Meaning |
|----------|---------|
| `adapter` | Southbound — ingests from field devices / protocols (OPC UA, Modbus, BACnet, …). |
| `processor` | Edge compute — transforms, aggregates, or analyzes data in flight. |
| `sink` | Northbound — forwards data to cloud, a historian, Kafka, etc. |

## Validation

CI validates `components.json` against `registry.schema.json` on every PR. Run it locally with any
JSON-Schema validator, e.g.:

```bash
python -m pip install check-jsonschema
check-jsonschema --schemafile registry.schema.json components.json
```
