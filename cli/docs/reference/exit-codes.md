# Reference — exit codes and diagnostics

The CLI separates *"it ran and found problems"* from *"it could not run"*, because CI needs to treat
those differently. Branch on the exit code, never on the message text.

## Exit codes

| Code | Name | Meaning |
|---|---|---|
| `0` | Ok | Success |
| `1` | Findings | The command ran and produced findings — validation failed, lint errors |
| `2` | Usage | The command was invoked incorrectly |
| `3` | Environment | A required external tool or environment prerequisite is missing |
| `4` | Internal | An unexpected internal error |
| `5` | NotImplemented | The verb is declared in the surface but not built in this binary |

Exit `5` is deliberate. A verb that exists in the surface but is not built says so with its own code,
so CI can tell *"this build cannot do that yet"* from *"you invoked it wrong"*. Today `deployment
lock`, `deployment diff`, and `studio serve` exit `5`.

## Diagnostic codes

Every finding carries a stable `EC****` code, so automation can pin behavior to a code rather than to
prose. Codes are grouped by family.

### `EC0xxx` — environment and toolchain

| Code | Meaning |
|---|---|
| `EC0001` | A required external tool is not on `PATH` |
| `EC0002` | An external tool is older than the minimum this CLI requires |

### `EC1xxx` — schema

| Code | Meaning |
|---|---|
| `EC1001` | The config does not validate against the canonical edgecommons config schema |
| `EC1002` | The config does not validate against the component's own `config.schema.json` |
| `EC1003` | The component publishes no config schema, so its own config is unvalidated |

`EC1003` is a warning, not an error. It exists so the tool states the limit of its coverage out loud
instead of implying validation it did not perform.

### `EC2xxx` — semantic

Rules a schema cannot express.

| Code | Meaning |
|---|---|
| `EC2001` | `--transport IPC` is valid only on `--platform GREENGRASS` |
| `EC2002` | A supervisord/HOST render requires `--platform HOST` |
| `EC2003` | A Kubernetes ConfigMap mount must not use `subPath` |
| `EC2004` | A hierarchical config lineage must be acyclic and ordered |
| `EC2005` | Secret values are forbidden; only `secret://` references |
| `EC2006` | A raw publish to a reserved UNS class is rejected |
| `EC2007` | A `CONFIG_COMPONENT` bootstrap loop |
| `EC2008` | A UNS identity/topic token is invalid |
| `EC2009` | The config source is not legal for the platform |

`EC2003` matters more than it looks: a `subPath` mount does not receive updates when the ConfigMap
changes, so a component would silently keep stale config forever.

### `EC3xxx` — artifact lint

Packaging mistakes that otherwise surface only at deploy time.

| Code | Meaning |
|---|---|
| `EC3001` | The recipe uses the `{COMPONENT_NAME}` placeholder, which GDK does not substitute |
| `EC3002` | An artifact `Permissions:` block is present; `CreateComponentVersion` rejects it |
| `EC3003` | Unsubstituted `<<...>>` placeholders remain |
| `EC3004` | `RequiresPrivilege: true` runs the component as root |
| `EC3005` | The recipe is not valid YAML |
| `EC3006` | `gdk-config.json` is missing or invalid |
| `EC3007` | `gdk-config.json`'s publish bucket is the unresolved sentinel, so it cannot publish |

### `EC4xxx` — templates

| Code | Meaning |
|---|---|
| `EC4001` | The template manifest is invalid |
| `EC4002` | The manifest references a file that is not in the template |
| `EC4003` | No template exists for the requested language/kind |
| `EC4004` | A component project declares no dependency manifest to operate on |
| `EC4005` | A Greengrass scaffold has no artifact bucket, so it cannot be published as-is |

### `EC5xxx` — deployment

| Code | Meaning |
|---|---|
| `EC5001` | The deployment definition fails its own schema (v1alpha1) |
| `EC5002` | A deployment semantic rule (S-1..S-9) is violated |
| `EC5003` | A rendered effective config fails the strict runtime config schema |
| `EC5004` | A node's platform identity diverges from its node key (runtime-identity consequence) |

`EC5004` is a warning. Diverging a node's thing name from its node key is legal, but the runtime's
device identity — and therefore every UNS topic the node publishes — resolves from the thing name, so
the consequence is surfaced rather than left to be discovered in production.

## Machine-readable output

`--json` returns the diagnostics structured, each with its code, file, and pointer where applicable:

```bash
edgecommons component validate --platform GREENGRASS --json
```

This is the supported way to consume findings. The human rendering is for humans and may change; the
codes are stable across releases.
