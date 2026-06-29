import { describe, it, expect } from "vitest";

import { Message, MessageBuilder } from "../src/message";

describe("Message / MessageBuilder", () => {
  it("envelope round-trips a byte-shape with snake_case header keys", () => {
    const msg = MessageBuilder.create("evt", "1.0.0")
      .withCorrelationId("corr-1")
      .withThingName("thing-1")
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

    expect(obj.tags).toEqual({ thing: "thing-1" });
    expect(obj.body).toEqual({ value: 42 });
  });

  it("serializes a Buffer body as a base64 string (#16)", () => {
    // A binary body travels as a base64 JSON string (portable cross-language interim), not the
    // non-portable `{ type: "Buffer", data: [...] }`. The canonical vector {0,1,2,254,255}
    // base64-encodes to "AAEC/v8=" — the same string Java/Python produce for the same bytes.
    const msg = MessageBuilder.create("bin", "1.0.0")
      .withPayload(Buffer.from([0, 1, 2, 254, 255]))
      .build();
    const obj = msg.toObject();
    expect(obj.body).toBe("AAEC/v8=");
    expect(JSON.parse(msg.toJSON()).body).toBe("AAEC/v8=");
  });

  it("preserves an explicit null map entry in the body (#15)", () => {
    // Parity with the Java fix: an explicit null object value round-trips as JSON null.
    const msg = MessageBuilder.create("evt", "1.0.0").withPayload({ present: 1, nullv: null }).build();
    expect(JSON.parse(msg.toJSON()).body).toEqual({ present: 1, nullv: null });
  });

  it("omits the thing tag when there is no thing name", () => {
    const msg = MessageBuilder.create("evt", "1.0.0").withPayload(1).build();
    const obj = msg.toObject();
    expect(obj.tags).toEqual({});
    expect("thing" in (obj.tags as object)).toBe(false);
  });

  it("includes reply_to only when set", () => {
    const msg = MessageBuilder.create("req", "1.0.0").withReplyTo("reply/here").build();
    const header = msg.toObject().header as Record<string, unknown>;
    expect(header.reply_to).toBe("reply/here");
  });

  it("JSON round-trips through fromWire", () => {
    const msg = MessageBuilder.create("evt", "2.0.0")
      .withThingName("t")
      .withTag("site", "f1")
      .withPayload({ a: [1, 2] })
      .build();

    const wire = msg.toJSON();
    const back = Message.fromWire(wire);
    expect(back.isRaw()).toBe(false);
    expect(back.header.name).toBe("evt");
    expect(back.header.version).toBe("2.0.0");
    expect(back.getBody()).toEqual({ a: [1, 2] });
    expect(back.tags).toEqual({ thing: "t", site: "f1" });
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

  it("fromWire delivers invalid JSON as a raw string", () => {
    const msg = Message.fromWire("not json {{{");
    expect(msg.isRaw()).toBe(true);
    expect(msg.getRaw()).toBe("not json {{{");
  });

  it("withConfig copies thingName and tags", () => {
    const msg = MessageBuilder.create("evt", "1.0.0")
      .withConfig({ thingName: "core-7", parsed: { tags: { region: "us" } } })
      .build();
    const obj = msg.toObject();
    expect(obj.tags).toEqual({ region: "us", thing: "core-7" });
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
