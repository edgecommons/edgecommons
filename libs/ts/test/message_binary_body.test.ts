import { describe, it, expect } from "vitest";

import { MAX_BINARY_BODY_BYTES, Message, MessageBodyCase, MessageBuilder } from "../src/message";

const MARKER = "_edgecommonsBinary";

/** Builds a message whose body is a raw binary marker object, as it arrives off the wire. */
function fromMarker(descriptor: unknown): Message {
  return MessageBuilder.create("Test", "1.0")
    .withStructuredPayload({ [MARKER]: descriptor })
    .build();
}

function descriptor(encoding?: unknown, length?: unknown, data?: unknown): Record<string, unknown> {
  const d: Record<string, unknown> = {};
  if (encoding !== undefined) d.encoding = encoding;
  if (length !== undefined) d.length = length;
  if (data !== undefined) d.data = data;
  return d;
}

describe("Message binary bodies", () => {
  it("round-trips an opaque payload through the marker envelope", () => {
    const payload = Buffer.from("hello bytes", "utf8");
    const msg = MessageBuilder.create("Test", "1.0").withOpaquePayload(payload).build();

    expect(msg.isBinaryBody()).toBe(true);
    expect(msg.getBodyCase()).toBe(MessageBodyCase.Opaque);
    expect(msg.getContentType()).toBe("application/octet-stream");
    expect(msg.getBinaryBody()).toEqual(payload);
    expect(msg.getOpaqueBody()).toEqual(payload);

    const body = msg.toObject().body as Record<string, Record<string, unknown>>;
    expect(body[MARKER].encoding).toBe("base64");
    expect(body[MARKER].length).toBe(payload.length);
    expect(Buffer.from(body[MARKER].data as string, "base64")).toEqual(payload);
  });

  it("carries the opaque body through a diagnostic view", () => {
    const payload = Buffer.from([1, 2, 3, 4]);
    const msg = MessageBuilder.create("Test", "1.0").withOpaqueBody(payload, "image/jpeg").build();

    const diagnostic = msg.toDiagnosticJson();

    expect(diagnostic.body_case).toBe(MessageBodyCase.Opaque);
    const body = diagnostic.body as Record<string, unknown>;
    expect(body.content_type).toBe("image/jpeg");
    expect(body.length).toBe(4);
    expect(typeof body.sha256).toBe("string");
  });

  it("rejects an oversized outbound binary body", () => {
    const msg = MessageBuilder.create("Test", "1.0")
      .withPayload(Buffer.alloc(MAX_BINARY_BODY_BYTES + 1))
      .build();

    expect(() => msg.getBinaryBody()).toThrow(/exceeds/);
  });

  it("has no binary view for a structured body", () => {
    const msg = MessageBuilder.create("Test", "1.0").withStructuredPayload({ a: 1 }).build();

    expect(msg.isBinaryBody()).toBe(false);
    expect(msg.getBinaryBody()).toBeUndefined();
    expect(msg.getOpaqueBody()).toBeUndefined();
    expect(msg.getBodyCase()).toBe(MessageBodyCase.Structured);
  });

  it("decodes a well-formed inbound marker", () => {
    const payload = Buffer.from([10, 20, 30]);
    const msg = fromMarker(descriptor("base64", payload.length, payload.toString("base64")));

    expect(msg.getBinaryBody()).toEqual(payload);
  });

  it("rejects a marker that is not an object", () => {
    expect(() => fromMarker("not-an-object").getBinaryBody()).toThrow(/must be an object/);
    expect(() => fromMarker(null).getBinaryBody()).toThrow(/must be an object/);
    expect(() => fromMarker([1, 2]).getBinaryBody()).toThrow(/must be an object/);
  });

  it("rejects a non-base64 encoding", () => {
    expect(() => fromMarker(descriptor("hex", 1, "AQ==")).getBinaryBody()).toThrow(/must be base64/);
  });

  it("rejects a missing or negative length", () => {
    expect(() => fromMarker(descriptor("base64", undefined, "AQ==")).getBinaryBody()).toThrow(
      /non-negative integer/,
    );
    expect(() => fromMarker(descriptor("base64", -1, "AQ==")).getBinaryBody()).toThrow(/non-negative integer/);
    expect(() => fromMarker(descriptor("base64", 1.5, "AQ==")).getBinaryBody()).toThrow(/non-negative integer/);
  });

  it("rejects a length beyond the cap", () => {
    expect(() => fromMarker(descriptor("base64", MAX_BINARY_BODY_BYTES + 1, "AQ==")).getBinaryBody()).toThrow(
      /exceeds/,
    );
  });

  it("rejects data that is missing or not strict base64", () => {
    expect(() => fromMarker(descriptor("base64", 1, undefined)).getBinaryBody()).toThrow(/not valid base64/);
    expect(() => fromMarker(descriptor("base64", 1, "!!not-base64!!")).getBinaryBody()).toThrow(/not valid base64/);
  });

  it("rejects a declared length that does not match the decoded data", () => {
    expect(() => fromMarker(descriptor("base64", 99, Buffer.from([1, 2]).toString("base64"))).getBinaryBody()).toThrow(
      /does not match/,
    );
  });

  it("accepts an empty binary body", () => {
    const msg = fromMarker(descriptor("base64", 0, ""));

    expect(msg.getBinaryBody()).toEqual(Buffer.alloc(0));
  });
});

describe("MessageBuilder body variants", () => {
  it("stamps the body case for every typed payload", () => {
    const body = { v: 1 };
    const cases: [Message, MessageBodyCase][] = [
      [MessageBuilder.create("T", "1").withStructuredBody(body).build(), MessageBodyCase.Structured],
      [
        MessageBuilder.create("T", "1").withSouthboundSignalUpdate(body).build(),
        MessageBodyCase.SouthboundSignalUpdate,
      ],
      [MessageBuilder.create("T", "1").withStateUpdate(body).build(), MessageBodyCase.StateUpdate],
      [MessageBuilder.create("T", "1").withConfigUpdate(body).build(), MessageBodyCase.ConfigUpdate],
      [MessageBuilder.create("T", "1").withMetricUpdate(body).build(), MessageBodyCase.MetricUpdate],
      [MessageBuilder.create("T", "1").withEvent(body).build(), MessageBodyCase.Event],
      [MessageBuilder.create("T", "1").withCommand(body).build(), MessageBodyCase.Command],
    ];

    for (const [msg, expected] of cases) {
      expect(msg.getBodyCase()).toBe(expected);
      expect(msg.getBody()).toEqual(body);
    }
  });

  it("withBodyCase overrides the inferred case", () => {
    const msg = MessageBuilder.create("T", "1")
      .withStructuredPayload({ v: 1 })
      .withBodyCase(MessageBodyCase.Event)
      .build();

    expect(msg.getBodyCase()).toBe(MessageBodyCase.Event);
  });

  it("carries content metadata on the envelope", () => {
    const schema = { name: "Signal", version: "1.0", content_type: "application/json" };
    const msg = MessageBuilder.create("T", "1")
      .withStructuredPayload({ a: 1 })
      .withContentType("application/json")
      .withContentEncoding("gzip")
      .withSchema(schema)
      .build();

    expect(msg.getContentType()).toBe("application/json");
    expect(msg.getContentEncoding()).toBe("gzip");
    expect(msg.getSchema()).toEqual(schema);

    const obj = msg.toObject();
    expect(obj.content_type).toBe("application/json");
    expect(obj.content_encoding).toBe("gzip");
    expect(obj.schema).toEqual(schema);
  });

  it("restores content metadata from an envelope object", () => {
    const original = MessageBuilder.create("T", "1")
      .withStructuredPayload({ a: 1 })
      .withContentType("application/json")
      .withContentEncoding("gzip")
      .withSchema({ name: "Signal", version: "1.0" })
      .build();

    const restored = Message.fromObject(original.toObject());

    expect(restored.getContentType()).toBe("application/json");
    expect(restored.getContentEncoding()).toBe("gzip");
    expect(restored.getSchema()).toEqual({ name: "Signal", version: "1.0" });
  });
});
