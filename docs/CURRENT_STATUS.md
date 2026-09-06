# Core implementation status

Reviewed 2026-09-06 against core `main` at `77518bc`. This is an implementation inventory from
current source and Git history. It does not replace dated test reports or claim a fresh runtime
validation run. The four language libraries declare version **0.5.0**.

| Surface | Implemented on the reviewed main | Remaining work or qualification |
| --- | --- | --- |
| Platforms | GREENGRASS/IPC, HOST/MQTT and KUBERNETES/MQTT; ConfigMap reload, Downward API identity, health, Prometheus and structured stdout logging | Additional platform profiles, a custom operator and PVC/StatefulSet streaming packaging are separate roadmap work. |
| Messaging | Protobuf `EdgeCommonsMessage` envelopes, structured and opaque bodies, local/northbound request/reply and reserved-class guards | Native configuration and AWS control-plane documents keep their own JSON/SDK formats. See [Southbound](SOUTHBOUND.md#2-the-normalized-telemetry-envelope) for the envelope projection convention. |
| UNS | Optional instance addressing (D-U28), component and instance scopes, data/events/app facades and structured log publishing | A fleet consumer must subscribe both scopes for each class it needs. |
| Commands | Required COMPONENT/INSTANCE/BOTH declarations, immediate and deferred handlers receiving addressing, describe metadata and instance state | Component handlers own configured-instance lookup and the single-instance fallback. See [scoped commands](platform/DESIGN-scoped-commands.md). |
| Configuration | Direct sources and CONFIG_COMPONENT lineage merge; canonical schema validation; candidate validators before initial activation and reload; numeric canonicalization | Automatic component-owned `config.schema.json` registration/enforcement (RM-014) is not implemented. Applications can register custom validation callbacks. |
| Streaming | Shared Rust durable/memory log, Kinesis/Kafka and callback sinks, native bindings and durable CloudWatch buffering | Native feature availability varies by published target; see [native delivery](NATIVE_CORE_DELIVERY.md). |
| CLI | Full language × kind template matrix; component lifecycle commands; deployment validate/lock/render/plan/release with HOST, Greengrass and Kubernetes renderers | `deployment diff` returns exit 5 on main. Container image builds and delivery remain runner responsibilities. |
| Studio | Context/scope navigation, scoped Overview/Config/Render views, global Releases, local draft creation/edit/list/status, advisory presence and semantic conflict review | Main opens a derived GitHub PR-create URL; it does not publish draft branches or create/merge PRs through a Git-host API. Several UI areas and global evidence provenance remain incomplete. |
| Release automation | Java/npm GitHub Packages publication, Python/Rust Git-ref distribution and native streamlog build/publication jobs | The CLI tag job still uses the retired Python build command; inspect/fix that workflow before using it to release the Rust CLI. Component release-index availability is a per-component fact. |

## Work outside main

These branches are recorded for continuity, not advertised as capabilities of the reviewed build:

- `feat/deployment-diff` (`fd52751`) implements deployment diff on a remote branch; no PR was found
  in the September 6 review.
- `feat/studio-apply-host` (`93fd01e`), [PR #77](https://github.com/edgecommons/edgecommons/pull/77),
  adds branch publication and PR opening. It remains unmerged.
- `design/http-transport-analysis` (`bd67cf2`) is pending design work, not a runtime transport.
- `docs/dallas-fixture-frozen-oracle`, [closed PR #68](https://github.com/edgecommons/edgecommons/pull/68),
  is not the accepted ownership baseline. Do not apply its wording solely because the branch exists.

## Evidence and validation

Implementation evidence lives in `libs/{java,python,rust,ts}`, `proto/edgecommons/v1`,
`cli/crates/ec-cli/src/main.rs`, `cli/crates/ec-studio/src/lib.rs` and the associated tests and Git
history. Reference pages describe these APIs; original design records retain their dated decisions.

Core CI enforces **92% line coverage** on its CI-testable library surface and **95% changed-line
coverage**. The CLI workspace gate is **90%**. Current workflow/build configuration is the
executable source for those thresholds. Live-infrastructure paths require the applicable MQTT,
Greengrass IPC and Kubernetes checks described in the [full interop runbook](../test-infra/interop/FULL_INTEROP_GREENGRASS_K8S.md).
Historical results remain evidence for their recorded commit/date; this inventory does not refresh them.
