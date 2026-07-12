import { describe, it, expect } from "vitest";

import { Message, MessageBodyCase, MessageBuilder, MessageIdentity } from "../src/message";

const IDENTITY = new MessageIdentity([{ level: "device", value: "gw-01" }], "opcua-adapter");

/** Encodes to the protobuf wire form and decodes it back, as the transports do. */
function roundTrip(msg: Message): Message {
  return Message.fromBytes(msg.toBytes());
}

function envelope(): MessageBuilder {
  return MessageBuilder.create("evt", "1.0.0").withIdentity(IDENTITY);
}

describe("protobuf codec — typed body cases", () => {
  it("round-trips a southbound signal update with samples", () => {
    const body = {
      signal: { id: "sig-1", name: "Temperature", address: { ns: 2, node: "Temp" } },
      samples: [
        {
          value: 21.5,
          quality: "GOOD",
          qualityRaw: 192,
          sourceTs: "2026-07-09T12:00:00Z",
          serverTs: "2026-07-09T12:00:01Z",
        },
      ],
    };

    const decoded = roundTrip(envelope().withSouthboundSignalUpdate(body).build());

    expect(decoded.getBodyCase()).toBe(MessageBodyCase.SouthboundSignalUpdate);
    const out = decoded.getBody() as Record<string, unknown>;
    expect(out.signal).toEqual({ id: "sig-1", name: "Temperature", address: { ns: 2, node: "Temp" } });
    const samples = out.samples as Record<string, unknown>[];
    expect(samples).toHaveLength(1);
    expect(samples[0].value).toBe(21.5);
    expect(samples[0].quality).toBe("GOOD");
    expect(samples[0].qualityRaw).toBe(192);
    expect(samples[0].sourceTs).toBe("2026-07-09T12:00:00Z");
    expect(samples[0].serverTs).toBe("2026-07-09T12:00:01Z");
  });

  it("round-trips a state update with instance connectivity", () => {
    const body = {
      status: "RUNNING",
      uptimeSecs: 90,
      instances: [{ instance: "line-1", connected: true, detail: "ok" }],
    };

    const decoded = roundTrip(envelope().withStateUpdate(body).build());

    expect(decoded.getBodyCase()).toBe(MessageBodyCase.StateUpdate);
    const out = decoded.getBody() as Record<string, unknown>;
    expect(out.status).toBe("RUNNING");
    expect(Number(out.uptimeSecs)).toBe(90);
    expect(out.instances).toEqual([{ instance: "line-1", connected: true, detail: "ok" }]);
  });

  it("round-trips a config update", () => {
    const decoded = roundTrip(envelope().withConfigUpdate({ config: { pollMs: 500, enabled: true } }).build());

    expect(decoded.getBodyCase()).toBe(MessageBodyCase.ConfigUpdate);
    const out = decoded.getBody() as Record<string, unknown>;
    expect(out.config).toEqual({ pollMs: 500, enabled: true });
  });

  it("round-trips a metric update with dimensions and values", () => {
    const body = {
      namespace: "EdgeCommons",
      metricName: "sys",
      timestampMs: 1_752_000_000_000,
      dimensions: { thing: "gw-01", component: "adapter" },
      values: [{ name: "cpu", value: 12.5, unit: "Percent", storageResolution: 60 }],
      largeFleetWorkaround: true,
    };

    const decoded = roundTrip(envelope().withMetricUpdate(body).build());

    expect(decoded.getBodyCase()).toBe(MessageBodyCase.MetricUpdate);
    const out = decoded.getBody() as Record<string, unknown>;
    expect(out.namespace).toBe("EdgeCommons");
    expect(out.metricName).toBe("sys");
    expect(Number(out.timestampMs)).toBe(1_752_000_000_000);
    expect(out.dimensions).toEqual({ thing: "gw-01", component: "adapter" });
    expect(out.values).toEqual([{ name: "cpu", value: 12.5, unit: "Percent", storageResolution: 60 }]);
    expect(out.largeFleetWorkaround).toBe(true);
  });

  it("round-trips an event", () => {
    const body = {
      severity: "WARN",
      type: "device.offline",
      message: "device went dark",
      timestamp: "2026-07-09T12:00:00Z",
      context: { retries: 3 },
      alarm: true,
      active: false,
    };

    const decoded = roundTrip(envelope().withEvent(body).build());

    expect(decoded.getBodyCase()).toBe(MessageBodyCase.Event);
    const out = decoded.getBody() as Record<string, unknown>;
    expect(out.severity).toBe("WARN");
    expect(out.type).toBe("device.offline");
    expect(out.message).toBe("device went dark");
    expect(out.timestamp).toBe("2026-07-09T12:00:00Z");
    expect(out.context).toEqual({ retries: 3 });
    expect(out.alarm).toBe(true);
    expect(out.active).toBe(false);
  });

  it("round-trips a command request, unwrapping a payload-only body", () => {
    const request = roundTrip(
      MessageBuilder.create("restart", "1.0.0")
        .withIdentity(IDENTITY)
        .withCommand({ verb: "restart", payload: { force: true } })
        .build(),
    );

    expect(request.getBodyCase()).toBe(MessageBodyCase.Command);
    // A request carrying nothing but a payload decodes back to the bare payload.
    expect(request.getBody()).toEqual({ force: true });
  });

  it("round-trips a command reply with its verb, status, result and error", () => {
    const ok = roundTrip(
      MessageBuilder.create("restart", "1.0.0")
        .withIdentity(IDENTITY)
        .withCommand({ verb: "restart", ok: true, result: { restarted: 1 } })
        .build(),
    );

    const okBody = ok.getBody() as Record<string, unknown>;
    expect(okBody.verb).toBe("restart");
    expect(okBody.ok).toBe(true);
    expect(okBody.result).toEqual({ restarted: 1 });

    const failed = roundTrip(
      MessageBuilder.create("restart", "1.0.0")
        .withIdentity(IDENTITY)
        .withCommand({
          verb: "restart",
          ok: false,
          error: { code: "BUSY", message: "already restarting", details: { attempt: 2 } },
        })
        .build(),
    );

    const failedBody = failed.getBody() as Record<string, unknown>;
    expect(failedBody.ok).toBe(false);
    expect(failedBody.error).toEqual({ code: "BUSY", message: "already restarting", details: { attempt: 2 } });
  });

  it("carries content metadata and tags across the wire", () => {
    const msg = envelope()
      .withStructuredPayload({ a: 1 })
      .withContentEncoding("gzip")
      .withSchema({ name: "Signal", version: "1.0", content_type: "application/json" })
      .withTag("site", "dallas")
      .withTag("line", 7)
      .build();

    const decoded = roundTrip(msg);

    expect(decoded.getContentEncoding()).toBe("gzip");
    expect(decoded.getSchema()).toEqual({ name: "Signal", version: "1.0", content_type: "application/json" });
    expect(decoded.tags).toEqual({ site: "dallas", line: 7 });
    expect(decoded.getIdentity()?.component).toBe("opcua-adapter");
  });

  it("carries an opaque body across the wire", () => {
    const payload = Buffer.from([0, 1, 2, 250, 255]);

    const decoded = roundTrip(envelope().withOpaqueBody(payload, "image/jpeg").build());

    expect(decoded.getBodyCase()).toBe(MessageBodyCase.Opaque);
    expect(decoded.getContentType()).toBe("image/jpeg");
    expect(decoded.getOpaqueBody()).toEqual(payload);
  });

  it("refuses to encode a raw message", () => {
    expect(() => Message.raw({ any: "value" }).toBytes()).toThrow(/requires a header/);
  });

  it("skips unknown fields of every supported wire type", () => {
    const base = envelope().withStructuredPayload({ a: 1 }).build().toBytes();
    const unknown = Buffer.from([
      0x90, 0x03, 0x01, // field 50, varint
      0x99, 0x03, 1, 2, 3, 4, 5, 6, 7, 8, // field 51, fixed64
      0xa5, 0x03, 1, 2, 3, 4, // field 52, fixed32
      0xaa, 0x03, 0x02, 0x01, 0x02, // field 53, length-delimited
    ]);

    const decoded = Message.fromBytes(Buffer.concat([base, unknown]));

    // Forward compatibility: unknown fields are skipped, the known ones still decode.
    expect(decoded.getBody()).toEqual({ a: 1 });
    expect(decoded.header.name).toBe("evt");
  });

  it("rejects an unknown field with an unsupported wire type", () => {
    const base = envelope().withStructuredPayload({ a: 1 }).build().toBytes();
    // Field 54, wire type 3 (start-group) — not supported by the decoder.
    const startGroup = Buffer.from([0xb3, 0x03]);

    expect(() => Message.fromBytes(Buffer.concat([base, startGroup]))).toThrow(/Malformed EdgeCommons protobuf/);
  });

  it("rejects an over-long varint", () => {
    const base = envelope().withStructuredPayload({ a: 1 }).build().toBytes();
    const overlong = Buffer.from([0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);

    expect(() => Message.fromBytes(Buffer.concat([base, overlong]))).toThrow(/Malformed EdgeCommons protobuf/);
  });

  it("rejects malformed protobuf bytes", () => {
    expect(() => Message.fromBytes(Buffer.from([0xff, 0xff, 0xff]))).toThrow(/Malformed EdgeCommons protobuf/);
    // A truncated length-delimited field: tag 1 (header), length 10, but no content.
    expect(() => Message.fromBytes(Buffer.from([0x0a, 0x0a]))).toThrow(/Malformed EdgeCommons protobuf/);
  });

  it("rejects a message whose header is incomplete", () => {
    expect(() => Message.fromBytes(Buffer.alloc(0))).toThrow(/requires header name and version/);
  });
});
