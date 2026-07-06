# EdgeCommons TypeScript — live-nucleus & AWS validation

Records the on-device GREENGRASS-mode and live-AWS validation of the TS library,
complementing the automated suites (`npm test` 163 unit/integration tests at >90%
coverage; the cross-language interop matrix 32/32 over MQTT). Performed against a
real AWS IoT Greengrass v2 **Nucleus 2.17.0** on Ubuntu `lab-5950x`
(thing `arn:aws:iot:us-east-1:162499689067:thing/lab-5950x`), components run as root.

> **Note (2026-07): this run predates the UNS hard cut.** The topics recorded below are the
> **legacy** scheme: the heartbeat is now the UNS `state` keepalive on
> `ecv1/{device}/{component}/main/state` (subscribe `ecv1/+/+/+/state`, not
> `edgecommons/<thing>/<comp>/heartbeat`), and the CONFIG_COMPONENT rendezvous moved to
> `ecv1/{device}/config/main/cmd/get-configuration`. The TS IoT-Core API family was also renamed
> `Iot → IoT` (`publishNorthbound`, `subscribeNorthbound`, …), and the envelope gained a top-level
> `identity` element (`tags.thing` removed). A future run re-validates against the UNS surface.

Harnesses (under `src/`, compiled to `dist/`):
- `edge_verify.ts` — full `EdgeCommonsBuilder` runtime; one component covers `GG_CONFIG`,
  `SHADOW`, and `CONFIG_COMPONENT` (config source from the recipe Run args). Checks
  config load, request/reply, raw, log metric target, heartbeat-over-IPC, the IoT
  Core bridge, and config hot-reload; writes a JSON result to `/tmp`.
- `config_provider.ts` — a peer config-manager component (answers `GetConfiguration`
  over IPC) for the `CONFIG_COMPONENT` test.
- `cw_verify.ts` — AWS-direct CloudWatch metric-target check (no nucleus).
- `ipc_verify.ts` — the original IPC smoke (connect / request-reply / raw / Java→TS
  heartbeat ingest).

Recipes: `com.mbreissi.edgecommons.TsEdgeVerify-1.0.1.yaml` (`-c GG_CONFIG`),
`-1.0.2.yaml` (`-c CONFIG_COMPONENT`), `-1.0.3.yaml` (`-c SHADOW TsEdgeVerify`),
`-1.0.4.yaml` (`-c SHADOW` with no name — exercises the sanitized default),
`com.mbreissi.edgecommons.TsConfigProvider-1.0.0.yaml`, `com.mbreissi.edgecommons.TsIpcVerify-1.0.2.yaml`.

## Results

| Capability | Result | How |
|---|---|---|
| Full `EdgeCommonsBuilder` lifecycle (GREENGRASS) | ✅ | component built end-to-end on the nucleus |
| `GG_CONFIG` load | ✅ | values read from the recipe `ComponentConfig` |
| `GG_CONFIG` hot-reload | ✅ | `greengrass-cli --update-config` (publish_interval 7→11) → `SubscribeToConfigurationUpdate` → in-process reload, no restart |
| `SHADOW` load (explicit name) | ✅ | named shadow `TsEdgeVerify` set in the cloud (`aws iot-data update-thing-shadow`) → ShadowManager 2.3.14 sync → IPC `GetThingShadow` → `state.desired.ComponentConfig` (publish_interval 9, site shadow-site); reported back |
| `SHADOW` load (default name, sanitized) | ✅ | `-c SHADOW` with NO name → the component-name default is sanitized to `com_edgecommons_TsEdgeVerify`; loaded the marker config (publish_interval 17, site shadow-default) from that named shadow. Exercises the dotted-name fix end-to-end (recipe `…TsEdgeVerify-1.0.4.yaml`). |
| `CONFIG_COMPONENT` load | ✅ | TS consumer ↔ TS `config_provider` request/reply over IPC on `edgecommons/<thing>/config/get/<full-component-name>` |
| Heartbeat over IPC | ✅ | library heartbeat received on `edgecommons/<thing>/TsEdgeVerify/heartbeat` (body cpu/memory) |
| Metric target — `log` | ✅ | EMF line written to the configured file |
| Metric target — `cloudwatch` | ✅ | `cw_verify` PutMetricData → metric `count` (ns `edgecommons-ts-verify`, dims category/coreName/token) visible in CloudWatch in ~15s. Backs heartbeat→cloudwatch (same target). |
| Request/reply over IPC | ✅ | correlation id round-trips, body echoed |
| Raw publish/ingest over IPC | ✅ | non-envelope payload delivered as raw |
| IoT Core bridge — publish (device→cloud) | ✅ | `publishNorthbound` succeeds via the nucleus |
| IoT Core bridge — subscribe (cloud→device) | ✅ | `subscribeNorthbound` + `aws iot-data publish` → component received `{cmd:ping,...}` |
| Cross-language Java→TS over IPC | ✅ | `ipc_verify` decoded the deployed Java component's heartbeat envelope |

## Notes / gotchas (for future runs)

- **IoT Core `QUOTA_EXCEEDED` (MQTT reasonCode 151).** The Nucleus multiplexes ALL
  components' IoT Core pub/sub over **one shared MQTT connection**, and AWS IoT Core
  has a fixed **per-connection subscription limit** (~50; not an account quota and
  not in Service Quotas). Subscriptions accumulate on that long-lived connection
  across deploy/remove cycles and from system components (ShadowManager adds several
  `$aws/.../shadow/...` subs). When subscribe returns `QUOTA_EXCEEDED`, restart the
  Nucleus (`systemctl restart greengrass`) to reset the connection — `subscribe`
  then succeeds (confirmed). **Therefore: test harnesses MUST unsubscribe before
  exiting** (and handle SIGTERM, which Greengrass sends on stop/remove) so they don't
  leak subscriptions onto the shared connection — `edge_verify`/`config_provider` do this.
- **AWS named-shadow names cannot contain dots** (`[a-zA-Z0-9:_-]+`). The SHADOW
  source defaults the shadow name to the (dotted) component name, so on-device you
  must pass an explicit valid name: `-c SHADOW <name>` (used `TsEdgeVerify`).
- **AWS credentials**: drive cloud-side steps from a host with valid creds (the dev
  workstation, account 162499689067); the lab box's own shell creds were invalid.
  Deployed components reach AWS via the device's TokenExchangeService role instead.
- **Cloud control-plane fleet ops are gated.** Group/thing-group deployments and
  `create-deployment` were blocked as shared-infra changes; ShadowManager was instead
  deployed **device-locally** via `greengrass-cli --merge "aws.greengrass.ShadowManager=2.3.14"`.
- The device was returned to its original component set after validation
  (test components + ShadowManager removed, the `TsEdgeVerify` cloud shadow deleted).
