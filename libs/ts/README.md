# ggcommons (TypeScript) — spike

A thin TypeScript port of the ggcommons messaging core, proving that a 4th-language
library interoperates on the same wire with the Java, Python, and Rust libraries.

**Scope (deliberately minimal):** the cross-language message envelope, a
STANDALONE-mode MQTT provider, and a GREENGRASS-mode IPC provider. It is *not* yet
feature-complete — config sources, metrics, heartbeat, and the dual-MQTT IoT-Core
leg are out of scope for the spike.

## What's here

| File | Purpose |
|------|---------|
| `src/message.ts` | `Message` / `MessageHeader` / `MessageTags` + fluent `MessageBuilder`. Byte-compatible wire shape (snake_case header keys, `thing` tag, `{raw}` for non-envelope payloads). |
| `src/standalone.ts` | `StandaloneProvider` over [`mqtt.js`](https://github.com/mqttjs/MQTT.js): connect/subscribe (block until confirmed), publish/publishRaw, request/reply (ephemeral `ggcommons/reply-…` topic + copied `correlation_id`), plus an MQTT `topicMatches` helper. |
| `src/ipc.ts` | `IpcProvider` over `aws-iot-device-sdk-v2`'s `greengrasscoreipc` client (GREENGRASS mode): the same public surface as `StandaloneProvider`, over Greengrass local pub/sub (`publishToTopic`/`subscribeToTopic`) and the IoT Core bridge. Envelope→`binaryMessage`, raw→`jsonMessage`, matching the Java/Python/Rust IPC providers. |
| `src/index.ts` | Public surface. |
| `src/interop_node.ts` | The cross-language interop node (compiled to `dist/interop_node.js`); joins the shared matrix in `test-infra/interop/` as the `ts` language. |

## Build

```bash
npm install
npm run build      # tsc -> dist/
```

## Greengrass IPC (GREENGRASS mode)

`src/ipc.ts` implements GREENGRASS-mode messaging over `aws-iot-device-sdk-v2`'s
`greengrasscoreipc` client. The JS SDK exposes only the **V1** IPC surface (manual
streaming operations — `subscribeToTopic(...).on('message', …)` + `.activate()`);
the simplified clientV2 used by Python/Java is Java/Python-only. The V1 surface is
fully capable of the local pub/sub + IoT-Core-bridge operations the library needs.
`IpcProvider` mirrors `StandaloneProvider`'s methods, so the transport-agnostic
parts (request/reply, the envelope) are identical across both modes.

The provider **compiles against the real SDK types** and reuses the exact wire
envelope already proven byte-identical across Java/Python/Rust/TS over MQTT.
Running it requires a live Greengrass nucleus: a deployed component supplies the
IPC env (`SVCUID`, the domain-socket path) and the recipe must grant
`aws.greengrass.ipc.pubsub` (and, for the bridge, `aws.greengrass.ipc.mqttproxy`)
`accessControl` for the topics used. `mqtt.js` (STANDALONE) needs none of this.

### Validated on a live Greengrass core (2026-06-19)

Deployed `IpcProvider` as a component (`deploy/com.ggcommons.TsIpcVerify-1.0.1.yaml`,
artifact = `src/ipc_verify.ts`) on a real AWS IoT Greengrass v2 nucleus (Ubuntu
`lab-5950x`, run-as root). All checks passed against the live nucleus:

- **`connected: true`** — the JS SDK's eventstream-RPC IPC client connected over
  the domain socket.
- **request/reply over IPC** — `correlation_match: true`, the responder echoed the
  request body (full request/reply traversed the nucleus).
- **raw publish/ingest over IPC** — a non-envelope payload arrived as `is_raw`.
- **cross-language Java → TS** — subscribed to
  `ggcommons/lab-5950x/JavaComponentSkeleton/heartbeat` and decoded the heartbeat
  **envelope published over IPC by the already-deployed Java ggcommons component**
  (`header.name="heartbeat"`, `thing="lab-5950x"`, tags `appId/line/shop/site/thing`,
  body `cpu/memory/files`) with the shared TS `Message` model.

This confirms the TS IPC binding interoperates with the other libraries over
Greengrass IPC, not just on the shared MQTT wire. See `deploy/README.md` to
reproduce.

## Interop

This library is exercised by the shared cross-language suite:

```bash
docker start ggcommons-emqx                       # local broker on :1883
cd ../../test-infra/interop && python -m pytest test_interop.py -v
```

It runs request/reply **and** raw publish/ingest for every ordered pair across
`{python, java, rust, ts}` — confirming `ts` is mutually intelligible with the
other three in both directions.
