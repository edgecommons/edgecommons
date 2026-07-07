/**
 * The TS loader for the `uns-test-vectors/{data,evt,app}.json` publish-facade conformance suite
 * (DESIGN-class-facades §5.3), mirroring the Java `UnsTestVectors.runDataCase`/`runEvtCase`/
 * `runAppCase` (`UnsTestVectorsLoaderTest`) and the `uns_vectors.test.ts` loader pattern.
 *
 * Replays every case through the LIVE `DataFacade`/`EventsFacade`/`AppFacade` (a fixed injected
 * clock, `RecordingMessagingService`, and a recording {@link StreamSink}) and asserts the pinned
 * `{topic, route, body[, partitionKey]}` — or `{throws: true}` — output. This is the
 * cross-language conformance gate for the body defaulting rules (quality → GOOD +
 * `qualityRaw:"unspecified"`, `serverTs`/`timestamp` → now, the `data` samples wrapper), the
 * `evt/{severity}/{type}` channel derivation, the `app` verbatim-body guarantee, and the
 * local/northbound/stream channel routing (DESIGN-class-facades §4).
 *
 * Existence-guarded like `uns_vectors.test.ts`: skips when the vector files are absent.
 */
import { existsSync, readFileSync } from "fs";
import { join } from "path";

import { describe, expect, it } from "vitest";

import { Config } from "../src/config/model";
import { EdgeCommonsError } from "../src/errors";
import { AppFacade } from "../src/facades/app_facade";
import { Channel } from "../src/facades/channel";
import { DataFacade } from "../src/facades/data_facade";
import { EventsFacade } from "../src/facades/events_facade";
import { qualityFromWire } from "../src/facades/quality";
import { severityFromWire } from "../src/facades/severity";
import { SignalUpdateBuilder } from "../src/facades/signal_update";
import type { StreamSink } from "../src/facades/stream_sink";
import { Message } from "../src/message";
import { Uns, UnsClass } from "../src/uns";
import { RecordingMessagingService } from "./_fakes";

const VECTORS = join(__dirname, "..", "..", "..", "uns-test-vectors");
const present =
  existsSync(join(VECTORS, "data.json")) && existsSync(join(VECTORS, "evt.json")) && existsSync(join(VECTORS, "app.json"));

function load(name: string): Record<string, unknown> {
  return JSON.parse(readFileSync(join(VECTORS, name), "utf8")) as Record<string, unknown>;
}

interface VectorCase {
  name: string;
  input: Record<string, unknown>;
  expected: Record<string, unknown>;
}

/** The pinned clock for the facade vectors: `serverTs`/`timestamp` resolve deterministically. */
const FACADE_NOW = "2026-07-01T12:00:00Z";
const FIXED_CLOCK = (): number => Date.parse(FACADE_NOW);

/** The single-level identity the publish-facade vectors bind to (device `gw-01`). Mirrors the Java `FACADE_IDENTITY`. */
function facadeConfig(): Config {
  return Config.fromValue("opcua-adapter", "gw-01", { component: {} });
}

/** True when `key` is present and not null/undefined (mirrors the Java loader's `has`). */
function has(obj: Record<string, unknown>, key: string): boolean {
  return key in obj && obj[key] !== null && obj[key] !== undefined;
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/**
 * Runs one `data.json` case: builds a `SignalUpdate` from the input, publishes it through a live
 * {@link DataFacade}, and reports what reached the wire (`topic`/`route`/`body`, plus
 * `partitionKey` for a stream route) or `{throws: true}` when the facade rejected it.
 */
async function runDataCase(input: Record<string, unknown>): Promise<Record<string, unknown>> {
  const instanceId = typeof input.instance === "string" ? input.instance : "kep1";
  const messaging = new RecordingMessagingService();
  const sink: { streamName?: string; partitionKey?: string; payload?: Buffer } = {};
  const streamSink: StreamSink = (streamName, partitionKey, _timestampMs, payload) => {
    sink.streamName = streamName;
    sink.partitionKey = partitionKey;
    sink.payload = payload;
  };
  const cfg = facadeConfig();
  const uns = new Uns(cfg.componentIdentity.withInstance(instanceId), false);
  const facade = new DataFacade(() => cfg, instanceId, uns, messaging, streamSink, FIXED_CLOCK);

  const signalId = has(input, "signalId") ? (input.signalId as string) : undefined;
  const builder = new SignalUpdateBuilder(signalId);
  if (has(input, "signalName")) builder.name(input.signalName as string);
  if (isObject(input.signalAddress)) builder.address(input.signalAddress);
  if (isObject(input.device)) builder.device(input.device);
  if (has(input, "signalPath")) builder.signalPath(input.signalPath as string);
  if (Array.isArray(input.samples)) {
    for (const sampleEl of input.samples as Array<Record<string, unknown>>) {
      const value = has(sampleEl, "value") ? sampleEl.value : undefined;
      builder.addSample(value, {
        quality: has(sampleEl, "quality") ? qualityFromWire(sampleEl.quality as string) : undefined,
        qualityRaw: has(sampleEl, "qualityRaw") ? (sampleEl.qualityRaw as string) : undefined,
        sourceTs: has(sampleEl, "sourceTs") ? (sampleEl.sourceTs as string) : undefined,
        serverTs: has(sampleEl, "serverTs") ? (sampleEl.serverTs as string) : undefined,
      });
    }
  }
  if (has(input, "override")) {
    builder.via(Channel.fromConfig(input.override as string)!);
  }

  try {
    await facade.publish(builder.build());
  } catch (e) {
    if (e instanceof EdgeCommonsError) {
      return { throws: true };
    }
    throw e;
  }

  if (messaging.published.length > 0) {
    const rec = messaging.published[0];
    return {
      topic: rec.topic,
      route: rec.qos === undefined ? "local" : "northbound",
      body: rec.message!.getBody(),
    };
  }
  const path = has(input, "signalPath") ? (input.signalPath as string) : (input.signalId as string);
  const envelope = Message.fromBytes(sink.payload!);
  return {
    topic: uns.topic(UnsClass.Data, facade.channelToken(path)),
    route: `stream:${sink.streamName}`,
    partitionKey: sink.partitionKey,
    body: envelope.getBody(),
  };
}

/** Runs one `evt.json` case through a live {@link EventsFacade}; reports `{topic, route, body}`. */
async function runEvtCase(input: Record<string, unknown>): Promise<Record<string, unknown>> {
  const cfg = facadeConfig();
  const messaging = new RecordingMessagingService();
  const uns = new Uns(cfg.componentIdentity, false);
  let facade = new EventsFacade(() => cfg, "main", uns, messaging, FIXED_CLOCK);
  if (has(input, "override")) {
    facade = facade.via(Channel.fromConfig(input.override as string)!);
  }

  const kind = input.kind as string;
  const type = input.type as string;
  const message = has(input, "message") ? (input.message as string) : undefined;
  const context = isObject(input.context) ? input.context : undefined;
  const severity = has(input, "severity") ? severityFromWire(input.severity as string) : undefined;

  switch (kind) {
    case "emit":
      if (severity === undefined) {
        await facade.emitInfo(type, message);
      } else {
        await facade.emit(severity, type, message, context);
      }
      break;
    case "raise":
      if (severity === undefined) {
        await facade.raiseAlarm(type, message, context);
      } else {
        await facade.raiseAlarm(type, message, context, severity);
      }
      break;
    case "clear":
      if (severity === undefined) {
        await facade.clearAlarm(type, context);
      } else {
        await facade.clearAlarm(type, context, severity);
      }
      break;
    default:
      throw new Error(`unknown evt kind: ${kind}`);
  }

  const rec = messaging.published[0];
  return {
    topic: rec.topic,
    route: rec.qos === undefined ? "local" : "northbound",
    body: rec.message!.getBody(),
  };
}

/** Runs one `app.json` case through a live {@link AppFacade}; reports `{topic, route, body}`. */
async function runAppCase(input: Record<string, unknown>): Promise<Record<string, unknown>> {
  const cfg = facadeConfig();
  const messaging = new RecordingMessagingService();
  const uns = new Uns(cfg.componentIdentity, false);
  const facade = new AppFacade(() => cfg, "main", uns, messaging);

  const name = input.name as string;
  const channel = input.channel as string;
  const body = isObject(input.body) ? input.body : {};
  const routing = has(input, "override") ? Channel.fromConfig(input.override as string) : undefined;
  await facade.publish(name, channel, body, routing);

  const rec = messaging.published[0];
  return {
    topic: rec.topic,
    route: rec.qos === undefined ? "local" : "northbound",
    body: rec.message!.getBody(),
  };
}

describe.skipIf(!present)("uns-test-vectors data/evt/app facade conformance", () => {
  it("data.json: every case matches the live DataFacade output", async () => {
    const doc = load("data.json");
    const cases = doc.cases as VectorCase[];
    expect(cases.length).toBeGreaterThan(0);
    for (const c of cases) {
      const result = await runDataCase(c.input);
      expect(result, `data case '${c.name}'`).toEqual(c.expected);
    }
  });

  it("evt.json: every case matches the live EventsFacade output", async () => {
    const doc = load("evt.json");
    const cases = doc.cases as VectorCase[];
    expect(cases.length).toBeGreaterThan(0);
    for (const c of cases) {
      const result = await runEvtCase(c.input);
      expect(result, `evt case '${c.name}'`).toEqual(c.expected);
    }
  });

  it("app.json: every case matches the live AppFacade output", async () => {
    const doc = load("app.json");
    const cases = doc.cases as VectorCase[];
    expect(cases.length).toBeGreaterThan(0);
    for (const c of cases) {
      const result = await runAppCase(c.input);
      expect(result, `app case '${c.name}'`).toEqual(c.expected);
    }
  });
});
