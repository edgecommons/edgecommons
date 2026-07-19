# <<COMPONENTNAME>>

A southbound **protocol adapter** (`<<COMPONENTFULLNAME>>`) written in TypeScript on top of the
`edgecommons` TypeScript library, generated from the EdgeCommons TypeScript protocol-adapter
template by the `edgecommons` CLI.

An adapter connects to devices, reads signals, and publishes them onto the UNS in the shape the rest
of the fleet expects — so that a consumer can chart a Modbus register and an OPC UA node without
knowing either protocol.

```text
  connect ──► poll ──► publish SouthboundSignalUpdate ──► report health
     ▲                                                         │
     └──────────── reconnect with backoff ◄────────────────────┘
```

It ships with a **`sim` backend**, so it runs end to end with no hardware. Replace it with your
protocol; nothing above the seam changes.

## Project layout

| Path | Purpose |
|------|---------|
| `src/main.ts` | Entry point: builds the `edgecommons` runtime from CLI args, runs the app. |
| `src/app.ts` | The adapter: one loop per device — connect, poll, publish, reconnect, report health, serve writes. |
| `src/device.ts` | **The seam you implement**: `DeviceBackend` / `DeviceSession`, plus the `sim` backend. |
| `test/` | Vitest suites for the invariants below (`npm test`). |
| `config.schema.json` | The component's own config (`component.global` + one device per instance). |
| `test-configs/` | Sample `config.json` + `standalone-messaging.json` for local runs. |
| `recipe.yaml`, `gdk-config.json`, `build.sh` | Greengrass packaging. |
| `Dockerfile`, `k8s/`, `compose.yaml`, `supervisor/` | Container / Kubernetes / HOST packaging. |

## Develop & run locally (HOST platform, MQTT transport)

No hardware needed — the `sim` backend answers. Start a local MQTT broker, then:

```bash
npm install
npm run build
node dist/main.js \
  --platform HOST --transport MQTT ./test-configs/standalone-messaging.json \
  -c FILE ./test-configs/config.json \
  -t my-thing

# watch the signals it publishes
mosquitto_sub -h localhost -p 1883 -t 'ecv1/+/+/+/data/#' -v
```

## The seam: `src/device.ts`

`DeviceSession` is one live connection to one device. Implement it once per protocol; the connection
lifecycle, backoff, publishing, and health above it never learn your protocol.

**The boundary rule, worth enforcing in review:** a backend knows protocols. It does **not** know
EdgeCommons topics, the UNS, message envelopes, or metrics. `src/device.ts` imports nothing from
`@edgecommons/edgecommons` — deliberately. If your `DeviceSession` starts importing the UNS or
messaging modules, the seam has leaked.

A **signal** is one data point (OPC UA calls it a "tag"; Modbus calls it a "register"). The word
"tag" is reserved in EdgeCommons for the envelope's business metadata, which is a different thing.

## What the archetype guarantees (and the tests that hold it there)

| Invariant | Why |
|---|---|
| Published through the **`data()` facade**, never a hand-built topic or body | The facade constructs `{device, signal, samples}`, mints `ecv1/{device}/{component}/{instance}/data/{signal}`, and stamps identity. A hand-rolled topic is a topic that will disagree with the envelope. |
| **Quality on every sample** (`GOOD \| BAD \| UNCERTAIN` + the native code in `qualityRaw`) | It lets a consumer gate on quality without knowing your protocol. |
| **A failed read is published as `BAD`, not swallowed** | A signal that silently stops updating is indistinguishable from one that is simply not changing. "I could not read this" is information; silence is not. |
| **Reconnect with exponential backoff + full jitter** | So a plant full of adapters does not reconnect in lockstep when a PLC reboots. A *permanent* failure (a bad endpoint, a rejected credential) backs off to the ceiling at once rather than hammering a device that will never answer. |
| **`southbound_health`, dimensioned by instance** | An operator sees a link go down without reading logs: the exact SOUTHBOUND.md §5 set — `connectionState`, `publishLatencyMs`, `pollLatencyMs`, `readErrors`, `staleSignals`, `reconnects`. |
| **Writes are ALLOW-LISTED by stable `signal.id`, and default to EMPTY** | An adapter that writes whatever it is asked to is a control-system vulnerability, not a convenience. "The caller was authorized" is not this component's judgement to make. |
| **A write is CONFIRMED** | The command reply is the *device's* answer, not "we sent it". |
| **One loop per instance** (one device) | Most device protocols are a single request/response channel; a write and a poll on two callers would interleave into nonsense. The loop serializes them. |

## The command surface

`ping` / `reload-config` / `get-configuration` are live with zero code (the library's inbox). This
adapter registers the full generic southbound family (`src/commands.ts`):

| Verb | Topic | Body |
|---|---|---|
| `sb/status` | `ecv1/{device}/{component}/cmd/sb/status` | `{"instance"?: "device-1"}` |
| `sb/read` | `ecv1/{device}/{component}/cmd/sb/read` | `{"signals": [{"signalId": "temperature-1"}]}` |
| `sb/write` | `ecv1/{device}/{component}/cmd/sb/write` | `{"writes": [{"signalId": "temperature-1", "value": 42}]}` |
| `sb/signals` | `ecv1/{device}/{component}/cmd/sb/signals` | `{}` |
| `sb/browse` | `ecv1/{device}/{component}/cmd/sb/browse` | `{"cursor"?: "...", "max"?: 200}` |
| `sb/pause` / `sb/resume` | `ecv1/{device}/{component}/cmd/sb/{pause,resume}` | `{}` |
| `reconnect` / `repoll` | `ecv1/{device}/{component}/cmd/{reconnect,repoll}` | `{}` |

The scope rides an `instance` body field rather than a topic segment (required once two or more
devices are configured), so one inbox serves every device this adapter owns. A write is refused
with `WRITE_NOT_ALLOWED` unless its `signalId` is on that instance's `writes.allow` list — which is
**empty by default**, making a fresh adapter read-only. Full payload shapes and error codes:
[docs/reference/messaging-interface.md](docs/reference/messaging-interface.md).

## Configuration

`component.instances[]` is **one device per entry**; `config.schema.json` is the contract:

```json
{
  "component": {
    "global": { "defaults": { "pollIntervalMs": 5000 } },
    "instances": [
      {
        "id": "device-1",
        "adapter": "sim",
        "connection": { "endpoint": "sim://device-1" },
        "pollIntervalMs": 5000,
        "writes": { "allow": [] }
      }
    ]
  }
}
```

`connection` is deliberately **open** (`additionalProperties: true`): every protocol needs different
keys — a unit id, a security policy, a slave address. Everything else stays
`additionalProperties: false`, so a typo'd key is caught at deploy time instead of silently ignored.

## CLI contract

- `-c/--config <SOURCE> [args...]` — `FILE | ENV | GG_CONFIG | SHADOW | CONFIG_COMPONENT` (default: from the resolved platform profile — GREENGRASS → GG_CONFIG, HOST → FILE, KUBERNETES → CONFIGMAP)
- `--platform <PLATFORM>` — `GREENGRASS | HOST | KUBERNETES | auto` (default `auto`)
- `--transport <TRANSPORT> [path]` — `IPC | MQTT [messaging_config.json]` (default: from the platform; IPC only valid on GREENGRASS)
- `-t/--thing <name>` — IoT Thing name

## Deploy to Greengrass

Packaged with the **GDK** using `gdk-config.json` and `recipe.yaml`; the custom build (`build.sh`)
runs `npm install` + `npm run build` and stages a ZIP artifact (`dist/` + `node_modules/` +
`package.json`).

```bash
gdk component build
gdk component publish
```

## Deploy to Kubernetes

Generated only when KUBERNETES is a selected target. Build the image from `./Dockerfile`, make it
available to the cluster, point `image:` at it, then apply the manifests:

```bash
docker build -t ghcr.io/<owner>/<<COMPONENTNAME>>:latest .
docker push ghcr.io/<owner>/<<COMPONENTNAME>>:latest    # or: kind load docker-image ...
kubectl apply -f k8s/
```

With `--platform auto` the library detects KUBERNETES from the ServiceAccount token, reads config
from the mounted ConfigMap (hot-reloaded on `kubectl apply`), uses the MQTT transport from that same
ConfigMap, and resolves identity from the Downward API — so the Deployment needs no args.

## The edgecommons dependency

`package.json` depends on the `edgecommons` library via a `file:` path dependency (filled in at
generation time, `--dep-source local`, the default). Build the sibling library first (`npm run build`
in `core/libs/ts`), since a `file:` dependency on a TypeScript package needs its `dist/` present.
Regenerate with `--dep-source registry` to depend on the published package instead.

## Docs and further reading

See [`docs/`](docs/) for the full Diátaxis set — a tutorial, how-to guides, an explanation of the
seam and its guarantees, sample configurations, and reference pages for configuration, the
messaging interface, metrics, and data types.

## Lockfile

This scaffold ships with no `package-lock.json` — a template cannot generate a *valid* lockfile
(the resolved graph depends on the dep-source and the moment you build), and doing so at scaffold
time would need network access, which the CLI deliberately avoids. Run `npm install` once, then
**commit `package-lock.json`** — `.gitignore` does not exclude it — so `npm ci` is reproducible in
CI and for every other contributor. `component validate` warns if it is missing.
