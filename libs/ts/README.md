# ggcommons (TypeScript) — spike

A thin TypeScript port of the ggcommons messaging core, proving that a 4th-language
library interoperates on the same wire with the Java, Python, and Rust libraries.

**Scope (deliberately minimal):** the cross-language message envelope and a
STANDALONE-mode MQTT provider. It is *not* yet feature-complete — config sources,
metrics, heartbeat, IPC/GREENGRASS mode, and the dual-MQTT IoT-Core leg are out of
scope for the spike.

## What's here

| File | Purpose |
|------|---------|
| `src/message.ts` | `Message` / `MessageHeader` / `MessageTags` + fluent `MessageBuilder`. Byte-compatible wire shape (snake_case header keys, `thing` tag, `{raw}` for non-envelope payloads). |
| `src/standalone.ts` | `StandaloneProvider` over [`mqtt.js`](https://github.com/mqttjs/MQTT.js): connect/subscribe (block until confirmed), publish/publishRaw, request/reply (ephemeral `ggcommons/reply-…` topic + copied `correlation_id`), plus an MQTT `topicMatches` helper. |
| `src/index.ts` | Public surface. |
| `src/interop_node.ts` | The cross-language interop node (compiled to `dist/interop_node.js`); joins the shared matrix in `test-infra/interop/` as the `ts` language. |

## Build

```bash
npm install
npm run build      # tsc -> dist/
```

## Greengrass IPC (GREENGRASS mode) — note

AWS ships `aws-iot-device-sdk-js-v2`, which can speak Greengrass IPC, but only the
**V1** IPC client surface (the V2 IPC client is Java/Python-only). STANDALONE/MQTT
uses pure-JS `mqtt.js` (no native build). Wiring up the IPC leg is the main piece
of work to take this from spike to parity.

## Interop

This library is exercised by the shared cross-language suite:

```bash
docker start ggcommons-emqx                       # local broker on :1883
cd ../../test-infra/interop && python -m pytest test_interop.py -v
```

It runs request/reply **and** raw publish/ingest for every ordered pair across
`{python, java, rust, ts}` — confirming `ts` is mutually intelligible with the
other three in both directions.
