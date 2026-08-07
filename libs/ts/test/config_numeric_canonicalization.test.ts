/**
 * The numeric contract at the configuration boundary — D-NC1…D-NC5
 * (`docs/platform/DESIGN-config-numeric-canonicalization.md`).
 *
 * A configuration store that round-trips JSON numbers through a 64-bit float — the Greengrass
 * config store is one — delivers `pollIntervalMs: 5000` as `5000.0`. Java, Python, and Rust
 * rewrite that back into integer form at their configuration intake boundary. **TypeScript runs no
 * such pass (D-NC3)**: JavaScript has a single number type, so the decoded `5000.0` *is* `5000` and
 * the distinction is unrepresentable. These tests pin that invariant so it stays true, and pin the
 * shared rule that a value the author did *not* write is never substituted (D-NC2) — the truncation
 * this file's `Math.trunc` reads used to perform.
 *
 * The documents below are parsed from JSON **text** on purpose: that is exactly what happens on the
 * wire. Every configuration source parses JSON, and the Greengrass IPC SDK itself does
 * `JSON.parse(payload_text)` on the eventstream payload before `getConfiguration()` returns it.
 */
import { tmpdir } from "os";
import { join } from "path";

import { describe, expect, it } from "vitest";

import { Config } from "../src/config/model";
import { EffectiveConfigPublisher } from "../src/config/effective_config";
import { asNonNegativeInteger, requireNonNegativeInteger } from "../src/config/numbers";
import { validate } from "../src/config/validation";
import { EdgeCommonsError } from "../src/errors";
import { Message, MessageBuilder } from "../src/message";
import type { IMessagingService } from "../src/messaging/types";

/**
 * The document as a store that keeps numbers as doubles hands it back: every integral number
 * written `x.0`, in the library sections, in `component.global`, and in `component.instances[]`.
 * `sampleRatio` and `deadbandPercent` are genuinely fractional and must stay that way.
 */
const STORE_SHAPED_TEXT = `{
  "heartbeat": { "intervalSecs": 10.0 },
  "health": { "enabled": true, "port": 8081.0 },
  "logging": {
    "fileLogging": { "enabled": true, "filePath": "/tmp/x.log", "maxFileSize": "10MB", "backupCount": 3.0 },
    "publish": { "enabled": true, "maxRecordBytes": 4096.0, "queue": { "maxRecords": 500.0, "onFull": "dropOldest" } }
  },
  "metricEmission": { "target": "prometheus", "targetConfig": { "intervalSecs": 15.0, "port": 9123.0 } },
  "tags": { "site": "dallas", "line": "3" },
  "component": {
    "global": { "requestTimeoutMs": 2000.0, "backoff": { "maxMs": 30000.0 }, "sampleRatio": 0.5 },
    "instances": [
      { "id": "plc-1", "pollIntervalMs": 5000.0, "slot": 0.0, "deadbandPercent": 1.5, "signals": ["a", "b"] },
      { "id": "plc-2", "pollIntervalMs": 1000.0, "slot": 1.0 }
    ]
  }
}`;

/** The same configuration as a file or ConfigMap carries it — the canonical form. */
const INTEGER_SHAPED_TEXT = STORE_SHAPED_TEXT.replace(/(\d)\.0\b/g, "$1");

const storeShaped = (): Record<string, unknown> => JSON.parse(STORE_SHAPED_TEXT);
const integerShaped = (): Record<string, unknown> => JSON.parse(INTEGER_SHAPED_TEXT);

const config = (raw: unknown): Config => Config.fromValue("com.example.MyComponent", "gw-01", raw);

/**
 * A strict consumer of a component subtree — the JavaScript stand-in for the `serde` struct a Rust
 * adapter deserializes an instance into (`pub poll_interval_ms: u64`), which is where the reported
 * failure surfaced: `invalid type: floating point 5000.0, expected u64`.
 */
function parseDeviceConfig(subtree: unknown): { id: string; pollIntervalMs: number; slot: number } {
  const raw = subtree as Record<string, unknown>;
  const int = (key: string): number => {
    const value = raw[key];
    if (typeof value !== "number" || !Number.isInteger(value)) {
      throw new Error(`invalid device config: invalid type: ${String(value)}, expected u64`);
    }
    return value;
  };
  return { id: String(raw.id), pollIntervalMs: int("pollIntervalMs"), slot: int("slot") };
}

describe("the store's double encoding is unrepresentable in JavaScript (D-NC3)", () => {
  it("a store-shaped document parses to the identical document as its integer form", () => {
    expect(storeShaped()).toEqual(integerShaped());
    // Not just deep-equal as values: it re-serializes byte-identically, so nothing downstream —
    // a re-encode, a hash, a byte-match report — can tell the two encodings apart.
    expect(JSON.stringify(storeShaped())).toBe(JSON.stringify(integerShaped()));
    expect(JSON.parse("5000.0")).toBe(5000);
    expect(JSON.stringify(5000.0)).toBe("5000");
  });
});

describe("the configuration a component receives is canonical", () => {
  it("passes the schema gate in store-shaped form", () => {
    expect(() => validate(storeShaped())).not.toThrow();
  });

  it("delivers integers through the real config intake, and leaves fractions alone", () => {
    const cfg = config(storeShaped());

    // The whole delivered document equals the canonical form the other three languages produce.
    expect(cfg.raw).toEqual(integerShaped());
    expect(JSON.stringify(cfg.raw)).toBe(JSON.stringify(integerShaped()));

    // component.instances[] — the subtree that was never covered and that killed the adapter.
    const instance = cfg.instance("plc-1") as Record<string, unknown>;
    expect(instance.pollIntervalMs).toBe(5000);
    expect(Number.isInteger(instance.pollIntervalMs)).toBe(true);
    expect(Number.isInteger(instance.slot)).toBe(true);
    expect(cfg.instanceIds()).toEqual(["plc-1", "plc-2"]);

    // component.global, including a nested object.
    const global = cfg.global() as Record<string, unknown>;
    expect(global.requestTimeoutMs).toBe(2000);
    expect((global.backoff as Record<string, unknown>).maxMs).toBe(30000);

    // A genuinely fractional value stays fractional, wherever it lives.
    expect(global.sampleRatio).toBe(0.5);
    expect(Number.isInteger(instance.deadbandPercent)).toBe(false);
    expect(instance.deadbandPercent).toBe(1.5);

    // Keys and non-numbers are untouched (D-NC4).
    expect(instance.signals).toEqual(["a", "b"]);
    expect(cfg.parsed.tags).toEqual({ site: "dallas", line: "3" });
    expect(cfg.parsed.tags.line).toBe("3");
  });

  it("parses the library sections out of a store-shaped document", () => {
    const cfg = config(storeShaped());
    expect(cfg.parsed.heartbeat.intervalSecs).toBe(10);
    expect(cfg.parsed.health.port).toBe(8081);
    expect(cfg.parsed.logging.fileLogging?.backupCount()).toBe(3);
    expect(cfg.parsed.logging.publish.maxRecordBytes).toBe(4096);
    expect(cfg.parsed.logging.publish.queue.maxRecords).toBe(500);
    expect(cfg.parsed.metricEmission.intervalSecs()).toBe(15);
    expect(cfg.parsed.metricEmission.prometheusPort()).toBe(9123);
  });

  it("a strict typed consumer parses the delivered instance subtree", () => {
    // The reported failure, reproduced without a device: the adapter's own parser refuses a float.
    const cfg = config(storeShaped());
    expect(parseDeviceConfig(cfg.instance("plc-1"))).toEqual({
      id: "plc-1",
      pollIntervalMs: 5000,
      slot: 0,
    });

    // And it still refuses a value that is genuinely fractional — the fix widens what parses, it
    // does not make a strict consumer lenient.
    const fractional = config({ component: { instances: [{ id: "plc-1", pollIntervalMs: 5000.5, slot: 0 }] } });
    expect(() => parseDeviceConfig(fractional.instance("plc-1"))).toThrow(/invalid type: 5000.5/);
  });

  it("the published effective (cfg) document is canonical", () => {
    const cfg = config(storeShaped());
    const publisher = new EffectiveConfigPublisher(() => cfg, undefined as unknown as IMessagingService);
    expect(JSON.stringify(publisher.redactedEffectiveConfig())).toBe(JSON.stringify(integerShaped()));
  });
});

describe("envelope tags encode the same EcValue type on every platform (D-NC5)", () => {
  const encode = (tags: Record<string, unknown>): Buffer =>
    MessageBuilder.create("N", "1.0")
      .withConfig(config({ tags }))
      .withPayload({})
      .withUuid("11111111-1111-1111-1111-111111111111")
      .withTimestamp("2026-08-07T00:00:00.000Z")
      // The correlation id is generated per build; pin it so the comparison is about the tags.
      .withCorrelationId("22222222-2222-2222-2222-222222222222")
      .build()
      .toBytes();

  it("an integral tag encodes as an integer whatever the store did to the document", () => {
    const fromStore = encode(JSON.parse('{ "line": 3.0, "ratio": 0.5 }'));
    const fromFile = encode(JSON.parse('{ "line": 3, "ratio": 0.5 }'));
    expect(fromStore.equals(fromFile)).toBe(true);

    // EcValue int64 is field 3 (varint): tag byte 0x18, then the varint 3.
    expect(fromStore.includes(Buffer.from([0x18, 0x03]))).toBe(true);
    // EcValue double is field 5 (64-bit): tag byte 0x29 then the IEEE-754 bytes. `line` must not
    // be encoded that way — that is the cross-platform skew this decision removes.
    const doubleThree = Buffer.alloc(9);
    doubleThree[0] = 0x29;
    doubleThree.writeDoubleLE(3, 1);
    expect(fromStore.includes(doubleThree)).toBe(false);

    // The genuinely fractional tag stays a double, and both round-trip.
    const roundTripped = Message.fromBytes(fromStore).tags ?? {};
    expect(roundTripped.line).toBe(3);
    expect(roundTripped.ratio).toBe(0.5);
  });

  it("the schema types every tag value as a string, bounding the skew", () => {
    // Mirrors the Java `aNumericTagIsRefusedByTheSchemaGate`: a numeric tag never survives a
    // schema-validated document on any store, so D-NC5 guards the paths that bypass the gate.
    expect(() => validate({ component: {}, tags: { line: 3 } })).toThrow(/schema validation/);
    expect(() => validate({ component: {}, tags: { line: "3" } })).not.toThrow();
  });
});

describe("a value the author did not write is never substituted (D-NC2)", () => {
  it("rejects a fractional value in an integer-typed library setting instead of truncating it", () => {
    expect(() => config({ heartbeat: { intervalSecs: 5.5 } })).toThrow(
      /configuration value 'heartbeat.intervalSecs' must be a whole number, but was 5.5/,
    );
    expect(() => config({ health: { port: 8081.5 } })).toThrow(
      /configuration value 'health.port' must be a whole number/,
    );
    expect(() => config({ logging: { publish: { maxRecordBytes: 4096.5 } } })).toThrow(
      /configuration value 'logging.publish.maxRecordBytes' must be a whole number/,
    );
    expect(() => config({ logging: { publish: { queue: { maxRecords: 500.5 } } } })).toThrow(
      /configuration value 'logging.publish.queue.maxRecords' must be a whole number/,
    );
    expect(() => config({ logging: { fileLogging: { backupCount: 2.5 } } })).toThrow(
      /configuration value 'logging.fileLogging.backupCount' must be a whole number/,
    );
  });

  it("rejects a negative value instead of clamping it to zero or to the default", () => {
    expect(() => config({ heartbeat: { intervalSecs: -5 } })).toThrow(
      /configuration value 'heartbeat.intervalSecs' must not be negative, but was -5/,
    );
    expect(() => config({ health: { port: -1 } })).toThrow(/must not be negative/);
  });

  it("rejects a non-number and an out-of-range value, and reports a config error", () => {
    expect(() => config({ heartbeat: { intervalSecs: "5" } })).toThrow(
      /configuration value 'heartbeat.intervalSecs' must be a number, but was "5"/,
    );
    expect(() => config({ health: { port: 2 ** 64 } })).toThrow(
      /configuration value 'health.port' is out of range for a 64-bit integer/,
    );
    try {
      config({ heartbeat: { intervalSecs: 5.5 } });
      throw new Error("expected a rejection");
    } catch (e) {
      expect(e).toBeInstanceOf(EdgeCommonsError);
      expect((e as EdgeCommonsError).kind).toBe("Config");
    }
  });

  it("falls back to the default — never to a truncated value — in the accessors with no error channel", () => {
    // 6.5 must NOT become 6, and 9123.5 must NOT become 9123: the defaults prove no truncation.
    const metric = config({
      metricEmission: { targetConfig: { intervalSecs: 6.5, port: 9123.5, buffer: { maxDiskBytes: 1.5 } } },
    }).parsed.metricEmission;
    expect(metric.intervalSecs()).toBe(5);
    expect(metric.prometheusPort()).toBe(9090);
    expect(metric.cloudwatchBuffer()?.maxDiskBytes).toBe(128 * 1024 * 1024);

    const negative = config({ metricEmission: { targetConfig: { intervalSecs: -5, port: -1 } } })
      .parsed.metricEmission;
    expect(negative.intervalSecs()).toBe(5);
    expect(negative.prometheusPort()).toBe(9090);
  });

  it("accepts an integral value in any numeric encoding", () => {
    expect(config({ heartbeat: { intervalSecs: 10 } }).parsed.heartbeat.intervalSecs).toBe(10);
    expect(config(JSON.parse('{ "heartbeat": { "intervalSecs": 10.0 } }')).parsed.heartbeat.intervalSecs).toBe(10);
    expect(config(JSON.parse('{ "heartbeat": { "intervalSecs": 1e1 } }')).parsed.heartbeat.intervalSecs).toBe(10);
    // Absent and null both fall through to the schema default.
    expect(config({ heartbeat: {} }).parsed.heartbeat.intervalSecs).toBe(5);
    expect(config({ heartbeat: { intervalSecs: null } }).parsed.heartbeat.intervalSecs).toBe(5);
  });
});

describe("the shared numeric readers", () => {
  it("requireNonNegativeInteger covers the shared rule", () => {
    expect(requireNonNegativeInteger(undefined, "x")).toBeUndefined();
    expect(requireNonNegativeInteger(null, "x")).toBeUndefined();
    expect(requireNonNegativeInteger(0, "x")).toBe(0);
    // `-0` reads as `0` (IEEE `-0 === 0`), matching the other languages' `-0.0` row.
    expect(requireNonNegativeInteger(-0, "x") === 0).toBe(true);
    expect(requireNonNegativeInteger(5, "x")).toBe(5);
    expect(requireNonNegativeInteger(JSON.parse("5.0"), "x")).toBe(5);
    expect(requireNonNegativeInteger(JSON.parse("5e3"), "x")).toBe(5000);
    // The 2^53 exact-integer ceiling and the top of the shared 64-bit window still read.
    expect(requireNonNegativeInteger(2 ** 53, "x")).toBe(9007199254740992);
    expect(requireNonNegativeInteger(2 ** 63, "x")).toBe(9223372036854775808);

    expect(() => requireNonNegativeInteger(5.5, "x")).toThrow(/must be a whole number, but was 5.5/);
    expect(() => requireNonNegativeInteger(-5, "x")).toThrow(/must not be negative, but was -5/);
    expect(() => requireNonNegativeInteger(2 ** 64, "x")).toThrow(/out of range for a 64-bit integer/);
    expect(() => requireNonNegativeInteger(1e20, "x")).toThrow(/out of range for a 64-bit integer/);
    expect(() => requireNonNegativeInteger(NaN, "x")).toThrow(/must be a finite number, but was NaN/);
    expect(() => requireNonNegativeInteger(Infinity, "x")).toThrow(/must be a finite number/);
    // Strings and booleans are never coerced, however numeric they look (D-NC4).
    expect(() => requireNonNegativeInteger("5000", "x")).toThrow(/must be a number, but was "5000"/);
    expect(() => requireNonNegativeInteger(true, "x")).toThrow(/must be a number, but was true/);
    expect(() => requireNonNegativeInteger({}, "x")).toThrow(/must be a number, but was an object/);
    expect(() => requireNonNegativeInteger([1], "x")).toThrow(/must be a number, but was an array/);
  });

  it("back the credentials and parameters sections too", async () => {
    // The same rule reaches the subsystem sections whose Rust counterparts carried their own
    // "Greengrass stores doubles" deserializers.
    const credentials = await import("../src/credentials/config");
    await expect(
      credentials.openFromConfig({ vault: { path: join(tmpdir(), "ec-numeric-vault"), keepVersions: 2.5 } }),
    ).rejects.toThrow(/configuration value 'credentials.vault.keepVersions' must be a whole number/);

    const parameters = await import("../src/parameters/config");
    await expect(
      parameters.openFromConfig({ source: { type: "env" }, refreshIntervalSecs: 300.5 }),
    ).rejects.toThrow(/configuration value 'parameters.refreshIntervalSecs' must be a whole number/);
  });

  it("asNonNegativeInteger applies the same rule with no error channel", () => {
    expect(asNonNegativeInteger(5)).toBe(5);
    expect(asNonNegativeInteger(JSON.parse("5.0"))).toBe(5);
    expect(asNonNegativeInteger(0)).toBe(0);
    for (const bad of [5.5, -5, -0.5, 2 ** 64, 1e20, NaN, Infinity, "5", true, null, undefined, {}, [5]]) {
      expect(asNonNegativeInteger(bad)).toBeUndefined();
    }
  });
});
