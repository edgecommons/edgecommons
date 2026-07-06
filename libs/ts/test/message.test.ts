import { describe, it, expect } from "vitest";

import { MAX_BINARY_BODY_BYTES, Message, MessageBuilder, MessageIdentity } from "../src/message";

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
    expect(msg.getIdentity()?.instance).toBe("main");
    expect(msg.getIdentity()?.component).toBe("opcua-adapter");
  });

  it("withInstance stamps the per-message instance token onto the config identity", () => {
    const msg = MessageBuilder.create("data", "1.0")
      .withConfig({ parsed: { tags: {} }, componentIdentity: IDENTITY })
      .withInstance("kep1")
      .build();
    expect(msg.getIdentity()?.instance).toBe("kep1");
  });

  it("withInstance rejects an empty token", () => {
    expect(() => MessageBuilder.create("d", "1").withInstance("")).toThrow(/non-empty/);
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
  it("computes path, device, and defaults instance to 'main'", () => {
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
    expect(id.instance).toBe("main");
    expect(id.hier).toHaveLength(3);
  });

  it("withInstance returns a copy with the new token and validates it", () => {
    const id = IDENTITY.withInstance("kep1");
    expect(id.instance).toBe("kep1");
    expect(IDENTITY.instance).toBe("main"); // original untouched
    expect(() => IDENTITY.withInstance("")).toThrow(/non-empty/);
  });

  it("constructor validates hier and component", () => {
    expect(() => new MessageIdentity([], "c")).toThrow(/at least one entry/);
    expect(() => new MessageIdentity([{ level: "", value: "v" }], "c")).toThrow(/level must be non-empty/);
    expect(() => new MessageIdentity([{ level: "l", value: "" }], "c")).toThrow(/value for level/);
    expect(() => new MessageIdentity([{ level: "l", value: "v" }], "")).toThrow(/component must be non-empty/);
  });

  it("toObject emits the canonical member order hier, path, component, instance", () => {
    const obj = IDENTITY.toObject();
    expect(Object.keys(obj)).toEqual(["hier", "path", "component", "instance"]);
    expect(obj.hier).toEqual([
      { level: "site", value: "dallas" },
      { level: "device", value: "gw-01" },
    ]);
  });

  it("fromObject is lenient: recomputes a missing path, defaults a missing instance", () => {
    const id = MessageIdentity.fromObject({
      hier: [{ level: "device", value: "gw-01" }],
      component: "comp",
    });
    expect(id?.path).toBe("gw-01");
    expect(id?.instance).toBe("main");
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
