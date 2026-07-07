# EdgeCommons Protobuf Messaging - Implementation Specification

> Status: IMPLEMENTATION SPECIFICATION.
>
> Binding design: [DESIGN-protobuf-messaging.md](DESIGN-protobuf-messaging.md).
> This document is the execution artifact for the protobuf hard cut. It exists
> to keep Java canonical work, the three language mirrors, and interop
> validation aligned.

## 1. Binding Invariants

All EdgeCommons UNS and request/reply messages are protobuf
`edgecommons.v1.EdgeCommonsMessage` bytes on:

- local MQTT;
- northbound MQTT;
- Greengrass IPC pub/sub.

Platform-native JSON and SDK-native messages remain native:

- `GG_CONFIG` uses Greengrass configuration APIs and update events;
- `SHADOW` uses shadow APIs and JSON shadow documents;
- `FILE`, `ENV`, and `CONFIGMAP` parse local JSON configuration;
- `CONFIG_COMPONENT` remains deferred for the future dedicated configuration
  management component port.

Passing builds are not completion. Completion requires fidelity to the binding
design, Java canonical validation, four-language parity, local MQTT interop, and
deployed Greengrass IPC interop.

## 2. Proto Ownership

Canonical proto sources live only under:

```text
proto/edgecommons/v1/
```

Required files:

```text
value.proto
message.proto
telemetry.proto
state.proto
config.proto
metrics.proto
event.proto
command.proto
```

All files use:

```proto
syntax = "proto3";
package edgecommons.v1;

option java_multiple_files = true;
option java_package = "com.mbreissi.edgecommons.proto.v1";
```

Descriptor generation:

```text
protoc -I proto --include_imports \
  --descriptor_set_out protobuf-test-vectors/edgecommons-v1.desc \
  proto/edgecommons/v1/*.proto
```

Java generated sources are build artifacts under:

```text
libs/java/target/generated-sources/protobuf/java
```

Java wrappers and codecs live under:

```text
libs/java/src/main/java/com/mbreissi/edgecommons/messaging/proto/
```

## 3. Required Schema

`value.proto`:

- `EcValue.oneof kind`: `null_value=1`, `bool_value=2`, `int_value=3`,
  `uint_value=4`, `double_value=5`, `string_value=6`, `bytes_value=7`,
  `list_value=8`, `map_value=9`.
- `EcList.values=1`.
- `EcMap.fields=1`.
- `NullValue.NULL_VALUE_UNSPECIFIED=0`.

`message.proto`:

- `EdgeCommonsMessage.header=1`.
- `EdgeCommonsMessage.identity=2`, optional.
- `EdgeCommonsMessage.tags=3`.
- `EdgeCommonsMessage.content_type=4`.
- `EdgeCommonsMessage.content_encoding=5`.
- `EdgeCommonsMessage.schema=6`.
- Body oneof:
  - `southbound_signal_update=20`;
  - `state_update=21`;
  - `config_update=22`;
  - `metric_update=23`;
  - `event=24`;
  - `command=25`;
  - `structured=30`;
  - `opaque=31`.
- `Header.name=1`, `version=2`, `timestamp_ms=3`, `uuid=4`,
  `correlation_id=5` optional, `reply_to=6` optional.
- `Identity.hier=1`, `path=2`, `component=3`, `instance=4`.
- `HierEntry.level=1`, `value=2`.
- `BodySchema.name=1`, `version=2`, `content_type=3`,
  `descriptor_ref=4`, `hash=5`.

`telemetry.proto`:

- `SouthboundSignalUpdate.signal=1`, `samples=2`, `extra=100`.
- `Signal.id=1`, `name=2`, `address=3`, `extra=100`.
- `Sample.value=1`, `quality=2`, `quality_raw=3`,
  `source_ts=4` optional, `source_ts_ms=5` optional,
  `server_ts=6` optional, `server_ts_ms=7` optional, `extra=100`.

`state.proto`:

- `StateUpdate.status=1`, `uptime_secs=2` optional, `instances=3`,
  `extra=100`.
- `InstanceConnectivity.instance=1`, `connected=2`, `detail=3` optional,
  `extra=100`.

`config.proto`:

- `ConfigUpdate.config=1`, `extra=100`.

`metrics.proto`:

- `MetricUpdate.namespace=1`, `metric_name=2`, `timestamp_ms=3`,
  `dimensions=4`, `values=5`, `large_fleet_workaround=6`,
  `emf_projection=20`, `extra=100`.
- `MetricValue.name=1`, `value=2`, `unit=3`, `storage_resolution=4`.

`event.proto`:

- `EventMessage.severity=1`, `type=2`, `message=3` optional,
  `timestamp=4`, `timestamp_ms=5` optional, `context=6`,
  `alarm=7` optional, `active=8` optional, `extra=100`.

`command.proto`:

- `CommandMessage.verb=1`, `payload=2`, `ok=3` optional, `result=4`,
  `error=5`, `extra=100`.
- `CommandError.code=1`, `message=2`, `details=100`.

Never reuse field numbers or removed names. Removed fields must be reserved.

## 4. Java Canonical Work Packages

### J1 - Proto Generation

- Add canonical proto files.
- Add Maven protobuf generation for Java generated sources.
- Add descriptor set generation for `protobuf-test-vectors/edgecommons-v1.desc`.
- Ensure generated source directory is included in Java compile.

### J2 - Message Codec

- Add `Message.toBytes()`.
- Add `Message.fromBytes(byte[])`.
- Add `Message.toDiagnosticJson()`.
- Add a wrapper codec under `messaging/proto`.
- Validate header, identity, tags, body case, content metadata, and size limits
  before encoding.
- Reject malformed protobuf instead of delivering it as a normal
  EdgeCommons message.

### J3 - Message Model

- `MessageHeader` stores canonical `timestampMs` as `long`.
- Deprecated string timestamp construction parses ISO-8601 to milliseconds.
- `getTimestamp()` may return an ISO diagnostic compatibility string.
- Add `getTimestampMs()`.
- `MessageTags` converts to and from `EcValue`.
- `Message` exposes body case, structured body, opaque body, content type,
  content encoding, and schema accessors.
- Existing `toDict()` remains diagnostic/reference output, not wire output.

### J4 - Builder API

Add:

- `withSouthboundSignalUpdate(...)`;
- `withStateUpdate(...)`;
- `withConfigUpdate(...)`;
- `withMetricUpdate(...)`;
- `withEvent(...)`;
- `withCommand(...)`;
- `withStructuredBody(...)`;
- `withOpaqueBody(byte[])`;
- `withContentType(...)`;
- `withContentEncoding(...)`;
- `withSchema(...)`.

Compatibility:

- `withPayload(JsonObject)` maps known EdgeCommons header names to typed bodies
  and all other JSON objects to `structured`.
- `withPayload(byte[])` maps to opaque with default
  `application/octet-stream`; keep it as a compatibility path but prefer
  `withOpaqueBody`.

### J5 - Provider Byte Boundary

- MQTT publish uses protobuf bytes.
- MQTT subscribe decodes protobuf bytes before invoking callbacks.
- Greengrass IPC publish uses `BinaryMessage.message` protobuf bytes.
- Greengrass IPC subscribe decodes `BinaryMessage.message` bytes on EdgeCommons
  topics and reply topics.
- IoT Core IPC publish/subscribe uses protobuf bytes for EdgeCommons messages.
- Platform JSON paths remain JSON/native.
- `publishRaw` stays for explicit non-EdgeCommons integrations and must not
  bypass reserved-topic guards.

### J6 - Facades And Reserved Publishers

- `data()` builds `southbound_signal_update`.
- Byte samples become `EcValue.bytes_value`.
- `source_ts` is never synthesized.
- `server_ts` may default to now and also fills `server_ts_ms` when possible.
- `events()` builds `event`.
- `app()` uses `structured` or `opaque`.
- `commands()` uses `command`.
- Reserved `state`, `cfg`, and `metric` publishers use typed bodies.
- `log` opaque chunks remain deferred.

### J7 - Streaming

Kinesis and Kafka sinks add:

```json
"payloadFormat": "json"
```

Allowed values:

- `json`, default: decode protobuf envelope and export canonical JSON.
- `protobuf`: export original protobuf envelope bytes.

Kafka should emit metadata as record headers. Kinesis JSON mode carries
equivalent metadata in the JSON projection.

## 5. Java Validation

Java unit tests must cover:

- structured telemetry encode/decode;
- nested byte-valued sample encode/decode;
- opaque body encode/decode with `content_type`;
- tags encode/decode, including scalar values and `_relay`;
- message/source/server timestamp semantics;
- request/reply correlation and reply topic;
- malformed protobuf rejection;
- diagnostic JSON base64 rendering for bytes;
- streaming JSON and protobuf payload format behavior.

Java integration sequence:

1. Run codec tests.
2. Run `mvn verify`.
3. Start EMQX with `docker compose -f test-infra/compose.yaml up -d`.
4. Run Java HOST MQTT tests proving payloads are protobuf bytes, not UTF-8 JSON.
5. Run the Java skeleton on HOST MQTT and decode observed payloads.
6. Deploy the Java skeleton to `lab-5950x` and verify Greengrass IPC protobuf
   bytes, request/reply, and native `GG_CONFIG`.

## 6. Mirror Handoff

Python, Rust, and TypeScript start after Java has stable:

- proto files;
- descriptor set;
- codec;
- diagnostic JSON;
- vectors;
- Java HOST and Greengrass validation evidence.

Each mirror must consume the canonical proto files and Java vectors. Language
APIs may be idiomatic, but observable behavior must match Java:

- body cases;
- validation errors;
- defaults;
- size limits;
- request/reply metadata;
- reserved-topic behavior;
- diagnostic JSON;
- provider byte boundary;
- platform JSON exceptions.

## 7. Final Interop Gates

Local MQTT interop must cover every ordered producer/consumer pair for:

- structured telemetry;
- nested byte-valued samples;
- opaque bodies;
- tags;
- raw/foreign payload policy;
- request/reply.

Greengrass IPC interop on `lab-5950x` must cover the same matrix with the four
language skeleton components.

AWS IoT Core Rules smoke must:

- upload `edgecommons-v1.desc`;
- decode `EdgeCommonsMessage` from `edgecommons/v1/message.proto` using the
  EdgeCommons `edgecommons.v1` descriptor package;
- assert header, identity, tags, topic, telemetry samples, and timestamps;
- verify byte values project as base64.

## 8. Risks And Required Escalation

Surface before deviating:

- deterministic protobuf map serialization may need language-specific wrappers;
- unknown-field preservation during bridge mutation may vary by language;
- `CONFIG_COMPONENT` has existing command plumbing but no new protobuf contract
  is approved;
- any inability to run HOST MQTT, Greengrass IPC, or IoT Core decode validation
  is a blocking validation gap, not completion.
