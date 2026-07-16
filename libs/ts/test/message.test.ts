import { describe, it, expect } from "vitest";
import { readFileSync } from "fs";
import { resolve } from "path";

import { MAX_BINARY_BODY_BYTES, Message, MessageBodyCase, MessageBuilder, MessageIdentity } from "../src/message";

const IDENTITY = new MessageIdentity(
  [
    { level: "site", value: "dallas" },
    { level: "device", value: "gw-01" },
  ],
  "opcua-adapter",
);

describe("Message / MessageBuilder", () => {
  it("envelope round-trips a byte-shape with snake_case header keys", () => {
    const msg = MessageBuilder.create("evt", "1.0.0")
      .withCorrelationId("corr-1")
      .withPayload({ value: 42 })
      .build();

    const obj = msg.toObject();
    const header = obj.header as Record<string, unknown>;
    expect(header.name).toBe("evt");
    expect(header.version).toBe("1.0.0");
    expect(header.correlation_id).toBe("corr-1");
    expect(typeof header.uuid).toBe("string");
    expect(typeof header.timestamp).toBe("string");
    // reply_to omitted when absent
    expect("reply_to" in header).toBe(false);

    // No tags stamped -> tags omitted from the wire object (matches Java's null-tags omission).
    expect("tags" in obj).toBe(false);
    expect(obj.body).toEqual({ value: 42 });
  });

  it("pins uuid and timestamp via the deterministic setters (D-U13)", () => {
    const msg = MessageBuilder.create("evt", "1.0")
      .withUuid("00000000-0000-4000-8000-000000000001")
      .withTimestamp("2026-07-01T12:00:00Z")
      .withCorrelationId("00000000-0000-4000-8000-000000000002")
      .build();
    const header = msg.toObject().header as Record<string, unknown>;
    expect(header.uuid).toBe("00000000-0000-4000-8000-000000000001");
    expect(header.timestamp).toBe("2026-07-01T12:00:00Z");
    expect(header.correlation_id).toBe("00000000-0000-4000-8000-000000000002");
  });

  it("serializes a Buffer body as a binary marker (#16)", () => {
    const msg = MessageBuilder.create("bin", "1.0.0")
      .withPayload(Buffer.from([0, 1, 2, 254, 255]))
      .build();
    const obj = msg.toObject();
    expect(obj.body).toEqual({
      _edgecommonsBinary: {
        encoding: "base64",
        length: 5,
        data: "AAEC/v8=",
      },
    });
    expect(JSON.parse(msg.toJSON()).body._edgecommonsBinary.data).toBe("AAEC/v8=");
    expect(msg.isBinaryBody()).toBe(true);
    expect(msg.getBinaryBody()).toEqual(Buffer.from([0, 1, 2, 254, 255]));
  });

  it("decodes inbound binary markers and validates length", () => {
    const msg = Message.fromObject({
      body: {
        _edgecommonsBinary: {
          encoding: "base64",
          length: 5,
          data: "AAEC/v8=",
        },
      },
    });
    expect(msg.isBinaryBody()).toBe(true);
    expect(msg.getBinaryBody()).toEqual(Buffer.from([0, 1, 2, 254, 255]));
    (msg.body as Record<string, Record<string, unknown>>)._edgecommonsBinary.length = 4;
    expect(() => msg.getBinaryBody()).toThrow(/length does not match/);
  });

  it("rejects oversized binary bodies", () => {
    const msg = MessageBuilder.create("bin", "1").withPayload(Buffer.alloc(MAX_BINARY_BODY_BYTES + 1)).build();
    expect(() => msg.toObject()).toThrow(/exceeds/);
  });

  it("preserves an explicit null map entry in the body (#15)", () => {
    // Parity with the Java fix: an explicit null object value round-trips as JSON null.
    const msg = MessageBuilder.create("evt", "1.0.0").withPayload({ present: 1, nullv: null }).build();
    expect(JSON.parse(msg.toJSON()).body).toEqual({ present: 1, nullv: null });
  });

  it("protobuf round-trips header, identity, tags, content metadata, schema, and structured body", () => {
    const msg = MessageBuilder.create("evt", "1.0")
      .withTimestampMs(1783360800000)
      .withUuid("00000000-0000-4000-8000-000000000010")
      .withCorrelationId("corr-10")
      .withReplyTo("reply/topic")
      .withIdentity(IDENTITY.withInstance("i1"))
      .withTags({ priority: 5, retention: "short", flag: true })
      .withContentEncoding("gzip")
      .withSchema({
        name: "AppPayload",
        version: "2",
        content_type: "application/json",
        descriptor_ref: "s3://bucket/app.desc",
        hash: "sha256:abc",
      })
      .withStructuredPayload({ present: 1, nullv: null, nested: { ok: true } })
      .build();

    const back = Message.fromBytes(msg.toBytes());

    expect(back.header.name).toBe("evt");
    expect(back.header.version).toBe("1.0");
    expect(back.header.timestamp_ms).toBe(1783360800000);
    expect(back.header.timestamp).toBe("2026-07-06T18:00:00.000Z");
    expect(back.header.uuid).toBe("00000000-0000-4000-8000-000000000010");
    expect(back.header.correlation_id).toBe("corr-10");
    expect(back.header.reply_to).toBe("reply/topic");
    expect(back.getIdentity()?.toObject()).toEqual(IDENTITY.withInstance("i1").toObject());
    expect(back.tags).toEqual({ flag: true, priority: 5, retention: "short" });
    expect(back.getContentEncoding()).toBe("gzip");
    expect(back.getSchema()).toEqual({
      name: "AppPayload",
      version: "2",
      content_type: "application/json",
      descriptor_ref: "s3://bucket/app.desc",
      hash: "sha256:abc",
    });
    expect(back.getBodyCase()).toBe(MessageBodyCase.Structured);
    expect(back.getBody()).toEqual({ nested: { ok: true }, nullv: null, present: 1 });
  });

  it("protobuf round-trips opaque bytes with explicit content_type", () => {
    const body = Buffer.from([0xff, 0xd8, 0xff, 0xe0]);
    const msg = MessageBuilder.create("FramePreview", "1.0")
      .withOpaquePayload(body, "image/jpeg")
      .build();

    const back = Message.fromBytes(msg.toBytes());

    expect(back.getBodyCase()).toBe(MessageBodyCase.Opaque);
    expect(back.getContentType()).toBe("image/jpeg");
    expect(back.getOpaqueBody()).toEqual(body);
    expect(back.toDiagnosticJson().body).toMatchObject({ content_type: "image/jpeg", length: 4 });
  });

  it("protobuf round-trips southbound signal bytes as EcValue bytes_value", () => {
    const msg = MessageBuilder.create("SouthboundSignalUpdate", "1.0")
      .withTimestampMs(1783360800000)
      .withSouthboundSignalUpdate({
        signal: { id: "camera-1/thumbnail", name: "thumbnail" },
        samples: [
          {
            value: Buffer.from([0, 1, 2, 254, 255]),
            quality: "GOOD",
            sourceTs: "2026-07-06T17:59:59.900Z",
            serverTs: "2026-07-06T18:00:00.000Z",
          },
        ],
      })
      .build();

    const back = Message.fromBytes(msg.toBytes());
    const body = back.getBody() as { signal: { id: string }; samples: Array<Record<string, unknown>> };

    expect(back.getBodyCase()).toBe(MessageBodyCase.SouthboundSignalUpdate);
    expect(body.signal.id).toBe("camera-1/thumbnail");
    expect(body.samples[0].value).toEqual({
      _edgecommonsBinary: { encoding: "base64", length: 5, data: "AAEC/v8=" },
    });
    expect(body.samples[0].sourceTsMs).toBe(1783360799900);
    expect(body.samples[0].serverTsMs).toBe(1783360800000);
  });

  it("reserved names infer typed protobuf body cases", () => {
    const state = Message.fromBytes(
      MessageBuilder.create("state", "1.0").withPayload({ status: "RUNNING", uptimeSecs: 42 }).build().toBytes(),
    );
    expect(state.getBodyCase()).toBe(MessageBodyCase.StateUpdate);
    expect(state.getBody()).toMatchObject({ status: "RUNNING", uptimeSecs: 42 });

    const cfg = Message.fromBytes(
      MessageBuilder.create("cfg", "1.0").withPayload({ config: { mode: "auto" } }).build().toBytes(),
    );
    expect(cfg.getBodyCase()).toBe(MessageBodyCase.ConfigUpdate);
    expect(cfg.getBody()).toEqual({ config: { mode: "auto" } });

    const metric = Message.fromBytes(
      MessageBuilder.create("Metric", "1.0")
        .withPayload({
          namespace: "EdgeCommons",
          metricName: "MessagesPublished",
          values: [{ name: "Count", value: 3, unit: "Count" }],
        })
        .build()
        .toBytes(),
    );
    expect(metric.getBodyCase()).toBe(MessageBodyCase.MetricUpdate);
    expect(metric.getBody()).toMatchObject({ metricName: "MessagesPublished" });

    const event = Message.fromBytes(
      MessageBuilder.create("evt", "1.0")
        .withPayload({ severity: "info", type: "door-open", message: "door opened" })
        .build()
        .toBytes(),
    );
    expect(event.getBodyCase()).toBe(MessageBodyCase.Event);
    expect(event.getBody()).toMatchObject({ type: "door-open" });
  });

  it("protobuf state instance connectivity defaults an omitted proto3 bool to disconnected", () => {
    // Java/protobuf omits a false proto3 bool on the wire. The diagnostic body must still carry the
    // contract's required `connected: false` value so consumers can render disconnected instances.
    const payload = Buffer.from(
      "0a0c0a0573746174651203312e30aa01180a0752554e4e494e471a0d0a0b70616c6c6574697a657231",
      "hex",
    );
    const state = Message.fromBytes(payload);

    expect(state.getBodyCase()).toBe(MessageBodyCase.StateUpdate);
    expect(state.getBody()).toEqual({
      status: "RUNNING",
      instances: [{ instance: "palletizer1", connected: false }],
    });
  });

  it("explicit command body preserves the component-facing payload", () => {
    const msg = MessageBuilder.create("ping", "1.0").withCommand({ status: "RUNNING" }).build();
    const back = Message.fromBytes(msg.toBytes());

    expect(back.getBodyCase()).toBe(MessageBodyCase.Command);
    expect(back.getBody()).toEqual({ status: "RUNNING" });
  });

  it("canonical protobuf vectors round-trip exact bytes", () => {
    const vectors = readFileSync(resolve("../../protobuf-test-vectors/messages.pb.hex"), "utf8")
      .trim()
      .split(/\r?\n/);
    for (const line of vectors) {
      if (!line || line.startsWith("#")) continue;
      const [id, hex] = line.split(" ", 2);
      const message = Message.fromBytes(Buffer.from(hex, "hex"));
      expect(message.toBytes().toString("hex"), id).toBe(hex);
    }
  });

  it("includes reply_to only when set", () => {
    const msg = MessageBuilder.create("req", "1.0.0").withReplyTo("reply/here").build();
    const header = msg.toObject().header as Record<string, unknown>;
    expect(header.reply_to).toBe("reply/here");
  });

  it("JSON round-trips through fromWire (tags carried, no thing stamp)", () => {
    const msg = MessageBuilder.create("evt", "2.0.0")
      .withTag("site", "f1")
      .withPayload({ a: [1, 2] })
      .build();

    const wire = msg.toJSON();
    const back = Message.fromWire(wire);
    expect(back.isRaw()).toBe(false);
    expect(back.header.name).toBe("evt");
    expect(back.header.version).toBe("2.0.0");
    expect(back.getBody()).toEqual({ a: [1, 2] });
    expect(back.tags).toEqual({ site: "f1" });
  });

  it("raw message via Message.raw serializes as {raw}", () => {
    const msg = Message.raw({ hello: "world" });
    expect(msg.isRaw()).toBe(true);
    expect(msg.getRaw()).toEqual({ hello: "world" });
    expect(msg.toObject()).toEqual({ raw: { hello: "world" } });
  });

  it("fromObject classifies a non-envelope value as raw", () => {
    const msg = Message.fromObject({ foo: "bar" });
    expect(msg.isRaw()).toBe(true);
    expect(msg.getRaw()).toEqual({ foo: "bar" });
  });

  it("fromObject classifies an object with body/header/tags as an envelope", () => {
    const msg = Message.fromObject({ body: { x: 1 } });
    expect(msg.isRaw()).toBe(false);
    expect(msg.getBody()).toEqual({ x: 1 });
  });

  it("fromObject treats a lone identity member as an envelope marker (§1.3)", () => {
    const msg = Message.fromObject({ identity: IDENTITY.toObject() });
    expect(msg.isRaw()).toBe(false);
    expect(msg.getIdentity()?.device).toBe("gw-01");
    expect(msg.getIdentity()?.path).toBe("dallas/gw-01");
  });

  it("a stray inbound 'thing' tag lands in the generic tag map (no legacy shim)", () => {
    const msg = Message.fromObject({ tags: { thing: "legacy-thing", site: "f1" }, body: 1 });
    expect(msg.tags).toEqual({ thing: "legacy-thing", site: "f1" });
  });

  it("fromWire delivers invalid JSON as a raw string", () => {
    const msg = Message.fromWire("not json {{{");
    expect(msg.isRaw()).toBe(true);
    expect(msg.getRaw()).toBe("not json {{{");
  });

  it("withConfig copies tags and stamps the component identity (§1.4)", () => {
    const msg = MessageBuilder.create("evt", "1.0.0")
      .withConfig({ parsed: { tags: { region: "us" } }, componentIdentity: IDENTITY })
      .build();
    const obj = msg.toObject();
    expect(obj.tags).toEqual({ region: "us" });
    // Canonical member order: header, identity, tags, body.
    expect(Object.keys(obj)).toEqual(["header", "identity", "tags", "body"]);
    // D-U28: a config-stamped message with no per-message instance is component-scoped.
    expect(msg.getIdentity()?.instance).toBeUndefined();
    expect(msg.getIdentity()?.component).toBe("opcua-adapter");
  });

  it("withInstance stamps the per-message instance token onto the config identity", () => {
    const msg = MessageBuilder.create("data", "1.0")
      .withConfig({ parsed: { tags: {} }, componentIdentity: IDENTITY })
      .withInstance("kep1")
      .build();
    expect(msg.getIdentity()?.instance).toBe("kep1");
  });

  it("fromObject leniently drops an identity whose instance is a reserved class token (D-U28)", () => {
    // The MessageIdentity constructor rejects a reserved-token instance; the lenient fromObject
    // parser catches that and returns undefined rather than propagating the throw.
    const id = MessageIdentity.fromObject({
      hier: [{ level: "device", value: "d" }],
      component: "c",
      instance: "state",
    });
    expect(id).toBeUndefined();
  });

  it("withInstance(empty/undefined) means component scope, not a throw (D-U28)", () => {
    const empty = MessageBuilder.create("d", "1")
      .withConfig({ parsed: { tags: {} }, componentIdentity: IDENTITY })
      .withInstance("")
      .build();
    expect(empty.getIdentity()?.instance).toBeUndefined();
    const none = MessageBuilder.create("d", "1")
      .withConfig({ parsed: { tags: {} }, componentIdentity: IDENTITY })
      .withInstance(undefined)
      .build();
    expect(none.getIdentity()?.instance).toBeUndefined();
  });

  it("withInstance rejects a reserved class token (D-U28)", () => {
    expect(() =>
      MessageBuilder.create("d", "1")
        .withConfig({ parsed: { tags: {} }, componentIdentity: IDENTITY })
        .withInstance("state")
        .build(),
    ).toThrow(/reserved UNS class token/);
  });

  it("withIdentity overrides the config identity verbatim (instance token not applied)", () => {
    const override = IDENTITY.withInstance("vec");
    const msg = MessageBuilder.create("data", "1.0")
      .withConfig({ parsed: { tags: {} }, componentIdentity: IDENTITY })
      .withInstance("kep1")
      .withIdentity(override)
      .build();
    expect(msg.getIdentity()).toBe(override);
    expect(msg.getIdentity()?.instance).toBe("vec");
  });

  it("no config and no override -> identity stays unset (bootstrap/raw case)", () => {
    const msg = MessageBuilder.create("GetConfiguration", "1.0").withPayload({ component: "c" }).build();
    expect(msg.getIdentity()).toBeUndefined();
    expect("identity" in msg.toObject()).toBe(false);
  });

  it("getCorrelationId / getReplyTo accessors", () => {
    const msg = MessageBuilder.create("a", "1")
      .withCorrelationId("c9")
      .withReplyTo("r/t")
      .build();
    expect(msg.getCorrelationId()).toBe("c9");
    expect(msg.getReplyTo()).toBe("r/t");
  });
});

describe("MessageIdentity", () => {
  it("computes path, device, and leaves instance undefined for component scope (D-U28)", () => {
    const id = new MessageIdentity(
      [
        { level: "site", value: "dallas" },
        { level: "zone", value: "zone-3" },
        { level: "device", value: "gw-01" },
      ],
      "comp",
    );
    expect(id.path).toBe("dallas/zone-3/gw-01");
    expect(id.device).toBe("gw-01");
    expect(id.instance).toBeUndefined(); // D-U28: no `main` default
    expect(id.hier).toHaveLength(3);
  });

  it("withInstance returns a copy with the new token; empty ⇒ component scope (D-U28)", () => {
    const id = IDENTITY.withInstance("kep1");
    expect(id.instance).toBe("kep1");
    expect(IDENTITY.instance).toBeUndefined(); // original untouched (component scope)
    expect(IDENTITY.withInstance("").instance).toBeUndefined(); // empty ⇒ component scope, no throw
  });

  it("rejects a reserved class token as an instance (D-U28)", () => {
    expect(() => IDENTITY.withInstance("cmd")).toThrow(/reserved UNS class token/);
    expect(() => new MessageIdentity([{ level: "device", value: "gw-01" }], "comp", "metric")).toThrow(
      /reserved UNS class token/,
    );
  });

  it("constructor validates hier and component", () => {
    expect(() => new MessageIdentity([], "c")).toThrow(/at least one entry/);
    expect(() => new MessageIdentity([{ level: "", value: "v" }], "c")).toThrow(/level must be non-empty/);
    expect(() => new MessageIdentity([{ level: "l", value: "" }], "c")).toThrow(/value for level/);
    expect(() => new MessageIdentity([{ level: "l", value: "v" }], "")).toThrow(/component must be non-empty/);
  });

  it("toObject omits instance for component scope, keeps canonical order when present (D-U28)", () => {
    // Component scope: the `instance` key is omitted entirely.
    const obj = IDENTITY.toObject();
    expect(Object.keys(obj)).toEqual(["hier", "path", "component"]);
    expect(obj.hier).toEqual([
      { level: "site", value: "dallas" },
      { level: "device", value: "gw-01" },
    ]);
    // Instance scope: the `instance` key appears last in canonical order.
    expect(Object.keys(IDENTITY.withInstance("kep1").toObject())).toEqual(["hier", "path", "component", "instance"]);
  });

  it("fromObject is lenient: recomputes a missing path, a missing instance ⇒ component scope (D-U28)", () => {
    const id = MessageIdentity.fromObject({
      hier: [{ level: "device", value: "gw-01" }],
      component: "comp",
    });
    expect(id?.path).toBe("gw-01");
    expect(id?.instance).toBeUndefined();
  });

  it("fromObject takes a present path as-is (publisher authoritative)", () => {
    const id = MessageIdentity.fromObject({
      hier: [{ level: "device", value: "gw-01" }],
      path: "custom/path",
      component: "comp",
      instance: "kep1",
    });
    expect(id?.path).toBe("custom/path");
    expect(id?.instance).toBe("kep1");
  });

  it("fromObject drops malformed identities with undefined (message still delivers)", () => {
    expect(MessageIdentity.fromObject(null)).toBeUndefined();
    expect(MessageIdentity.fromObject("nope")).toBeUndefined();
    expect(MessageIdentity.fromObject({})).toBeUndefined();
    expect(MessageIdentity.fromObject({ hier: [] })).toBeUndefined();
    expect(MessageIdentity.fromObject({ hier: ["x"], component: "c" })).toBeUndefined();
    expect(MessageIdentity.fromObject({ hier: [{ level: "d" }], component: "c" })).toBeUndefined();
    expect(MessageIdentity.fromObject({ hier: [{ level: "d", value: "v" }] })).toBeUndefined();
  });

  it("a malformed inbound identity is dropped but the message still delivers", () => {
    const msg = Message.fromObject({
      header: { name: "x", version: "1" },
      identity: { hier: "not-an-array" },
      body: { ok: true },
    });
    expect(msg.isRaw()).toBe(false);
    expect(msg.getIdentity()).toBeUndefined();
    expect(msg.getBody()).toEqual({ ok: true });
  });

  it("identity round-trips through the wire", () => {
    const msg = MessageBuilder.create("data", "1.0").withIdentity(IDENTITY.withInstance("i2")).withPayload(1).build();
    const back = Message.fromWire(msg.toJSON());
    expect(back.getIdentity()?.toObject()).toEqual(IDENTITY.withInstance("i2").toObject());
  });
});
