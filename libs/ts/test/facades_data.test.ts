/**
 * Deterministic unit tests for {@link DataFacade} — the `data()` publish facade
 * (DESIGN-class-facades §2.1, D2/D5): the `SouthboundSignalUpdate` body construction + defaulting
 * (quality → GOOD + `qualityRaw:"unspecified"`, `serverTs` → now), the missing-`signal.id`
 * reject, the raw escape hatch, and the local/northbound/stream channel routing. Time is a fixed
 * injected clock so `serverTs` defaults are pinned. Mirrors the Java `DataFacadeTest`.
 */
import { describe, expect, it, vi } from "vitest";

import { Config } from "../src/config/model";
import { GgError } from "../src/errors";
import { Channel } from "../src/facades/channel";
import { DataFacade } from "../src/facades/data_facade";
import { Quality } from "../src/facades/quality";
import type { StreamSink } from "../src/facades/stream_sink";
import { SignalUpdateBuilder } from "../src/facades/signal_update";
import { Qos } from "../src/messaging/types";
import { Uns } from "../src/uns";
import { RecordingMessagingService } from "./_fakes";

const NOW = "2026-07-01T12:00:00Z";
const CLOCK_MS = Date.parse(NOW);
const FIXED_CLOCK = (): number => CLOCK_MS;

function config(raw: Record<string, unknown> = { component: {} }): Config {
  return Config.fromValue("opcua-adapter", "gw-01", raw);
}

function makeFacade(opts?: {
  raw?: Record<string, unknown>;
  streamSink?: StreamSink;
  messaging?: RecordingMessagingService;
}): { facade: DataFacade; messaging: RecordingMessagingService; cfg: Config } {
  const cfg = config(opts?.raw);
  const messaging = opts?.messaging ?? new RecordingMessagingService();
  const uns = new Uns(cfg.componentIdentity.withInstance("kep1"), false);
  const facade = new DataFacade(() => cfg, "kep1", uns, messaging, opts?.streamSink, FIXED_CLOCK);
  return { facade, messaging, cfg };
}

function lastBody(messaging: RecordingMessagingService): Record<string, unknown> {
  const rec = messaging.published[messaging.published.length - 1];
  return rec.message!.getBody() as Record<string, unknown>;
}

function firstSample(body: Record<string, unknown>): Record<string, unknown> {
  return (body.samples as Record<string, unknown>[])[0];
}

describe("DataFacade", () => {
  describe("defaulting", () => {
    it("quality defaults to GOOD with the unspecified marker and serverTs now", async () => {
      const { facade, messaging } = makeFacade();
      await facade.publish("temp", 21.5);

      const rec = messaging.published[0];
      expect(rec.topic).toBe("ecv1/gw-01/opcua-adapter/kep1/data/temp");
      expect(rec.qos, "LOCAL route is the default (no QOS)").toBeUndefined();
      const sample = firstSample(lastBody(messaging));
      expect(sample.value).toBe(21.5);
      expect(sample.quality).toBe("GOOD");
      expect(sample.qualityRaw, "a defaulted quality carries the synthetic marker").toBe("unspecified");
      expect(sample.serverTs).toBe(NOW);
      expect(sample.sourceTs, "sourceTs is never synthesized").toBeUndefined();
    });

    it("an explicit quality is not marked unspecified", async () => {
      const { facade, messaging } = makeFacade();
      await facade.publish("temp", 0, Quality.Bad);

      const sample = firstSample(lastBody(messaging));
      expect(sample.quality).toBe("BAD");
      expect(sample.qualityRaw, "an explicit quality with no qualityRaw stays unmarked").toBeUndefined();
    });

    it("an explicit qualityRaw is passed through verbatim", async () => {
      const { facade, messaging } = makeFacade();
      await facade
        .signal("temp")
        .addSample(21.5, { quality: Quality.Good, qualityRaw: "Good", sourceTs: "2026-07-01T11:00:00Z", serverTs: "2026-07-01T11:00:01Z" })
        .publish();

      const sample = firstSample(lastBody(messaging));
      expect(sample.qualityRaw).toBe("Good");
      expect(sample.sourceTs).toBe("2026-07-01T11:00:00Z");
      expect(sample.serverTs, "a caller-supplied serverTs is not overwritten by now").toBe("2026-07-01T11:00:01Z");
    });

    it("the fluent builder constructs the full southbound body", async () => {
      const { facade, messaging } = makeFacade();
      await facade
        .signal("ns=2;s=Line1.Temp")
        .name("Line 1 Temperature")
        .address({ ns: 2, nodeId: "Line1.Temp" })
        .device("opcua", "kep1", "opc.tcp://host:4840")
        .addSample(21.5)
        .signalPath("press12/temperature")
        .publish();

      const rec = messaging.published[0];
      expect(rec.topic).toBe("ecv1/gw-01/opcua-adapter/kep1/data/press12/temperature");
      const body = lastBody(messaging);
      expect((body.device as Record<string, unknown>).adapter).toBe("opcua");
      expect((body.signal as Record<string, unknown>).id).toBe("ns=2;s=Line1.Temp");
      expect((body.signal as Record<string, unknown>).name).toBe("Line 1 Temperature");
      expect(((body.signal as Record<string, unknown>).address as Record<string, unknown>).ns).toBe(2);
    });

    it("batch samples are published in order", async () => {
      const { facade, messaging } = makeFacade();
      await facade.signal("flow").addSample(1.0).addSample(2.0, { quality: Quality.Uncertain }).publish();

      expect((lastBody(messaging).samples as unknown[]).length).toBe(2);
    });
  });

  describe("rejects (the only hard failures)", () => {
    it("a missing signal.id is rejected", async () => {
      const { facade, messaging } = makeFacade();
      const update = new SignalUpdateBuilder(undefined).addSample(1.0).build();
      await expect(facade.publish(update)).rejects.toThrow(GgError);
      expect(messaging.published, "nothing reaches the wire").toHaveLength(0);
    });

    it("empty samples is rejected", async () => {
      const { facade } = makeFacade();
      const update = new SignalUpdateBuilder("temp").build();
      await expect(facade.publish(update)).rejects.toThrow(GgError);
    });

    it("a quality-only sample with no value is rejected", async () => {
      const { facade } = makeFacade();
      const update = new SignalUpdateBuilder("temp").addSample(undefined, { quality: Quality.Bad }).build();
      await expect(facade.publish(update)).rejects.toThrow(GgError);
    });
  });

  describe("channel sanitization", () => {
    it("the channel path is sanitized", async () => {
      const { facade, messaging } = makeFacade();
      await facade.publish("a+b", 1.0);
      expect(messaging.published[0].topic).toBe("ecv1/gw-01/opcua-adapter/kep1/data/a_b");
    });

    it("a multi-token signal path becomes multiple channel tokens", async () => {
      const { facade, messaging } = makeFacade();
      await facade.publish("a/b", 1.0);
      expect(messaging.published[0].topic).toBe("ecv1/gw-01/opcua-adapter/kep1/data/a/b");
    });
  });

  describe("raw escape hatch", () => {
    it("publishes the body verbatim", async () => {
      const { facade, messaging } = makeFacade();
      const raw = { anything: "goes", n: 7 };
      await facade.publishBody("custom", raw);

      const rec = messaging.published[0];
      expect(rec.topic).toBe("ecv1/gw-01/opcua-adapter/kep1/data/custom");
      expect(rec.message!.getBody(), "the escape hatch applies no defaulting").toEqual(raw);
    });
  });

  describe("channel routing", () => {
    it("a northbound override routes to IoT Core", async () => {
      const { facade, messaging } = makeFacade();
      await facade.signal("temp").addSample(21.5).via(Channel.NORTHBOUND).publish();

      const rec = messaging.published[0];
      expect(rec.qos, "northbound uses publishToIoTCore").toBe(Qos.AtLeastOnce);
    });

    it("a stream override appends to the stream with the signal.id partition key", async () => {
      const recorded: { streamName?: string; partitionKey?: string; timestampMs?: number; payload?: Buffer } = {};
      const sink: StreamSink = (streamName, partitionKey, timestampMs, payload) => {
        recorded.streamName = streamName;
        recorded.partitionKey = partitionKey;
        recorded.timestampMs = timestampMs;
        recorded.payload = payload;
      };
      const { facade, messaging } = makeFacade({ streamSink: sink });
      await facade.signal("ns=2;s=Line1.Temp").addSample(21.5).via(Channel.stream("hot")).publish();

      expect(messaging.published, "the record went to the stream, not the bus").toHaveLength(0);
      expect(recorded.streamName).toBe("hot");
      expect(recorded.partitionKey, "partition key is the stable signal.id").toBe("ns=2;s=Line1.Temp");
      expect(recorded.timestampMs).toBe(CLOCK_MS);
      const env = JSON.parse(recorded.payload!.toString("utf8")) as Record<string, unknown>;
      const envBody = env.body as Record<string, unknown>;
      expect((envBody.signal as Record<string, unknown>).id, "the streamed payload is the same enriched envelope").toBe(
        "ns=2;s=Line1.Temp",
      );
    });

    it("a stream route falls back to local when no streaming is configured", async () => {
      // makeFacade() with no streamSink -> streaming not configured.
      const { facade, messaging } = makeFacade();
      await facade.signal("temp").addSample(21.5).via(Channel.stream("hot")).publish();

      expect(messaging.published, "readiness/no-streaming -> local").toHaveLength(1);
      expect(messaging.published[0].qos).toBeUndefined();
    });

    it("a configured instance publish.channel routes without an override", async () => {
      const { facade, messaging } = makeFacade({
        raw: { component: { instances: [{ id: "kep1", publish: { channel: "northbound" } }] } },
      });
      await facade.publish("temp", 21.5);

      expect(messaging.published[0].qos, "config publish.channel=northbound routes northbound").toBe(Qos.AtLeastOnce);
    });

    it("a configured global publish.channel is the fallback default", async () => {
      const { facade, messaging } = makeFacade({
        raw: { component: { global: { publish: { channel: "northbound" } } } },
      });
      await facade.publish("temp", 21.5);

      expect(messaging.published[0].qos).toBe(Qos.AtLeastOnce);
    });

    it("a per-call override wins over the config default", async () => {
      const { facade, messaging } = makeFacade({
        raw: { component: { instances: [{ id: "kep1", publish: { channel: "northbound" } }] } },
      });
      await facade.signal("temp").addSample(21.5).via(Channel.LOCAL).publish();

      expect(messaging.published[0].qos, "an explicit via(LOCAL) beats the config northbound default").toBeUndefined();
    });

    it("resolveChannel precedence", () => {
      const { facade } = makeFacade();
      expect(facade.resolveChannel(Channel.NORTHBOUND)).toEqual(Channel.NORTHBOUND);
      expect(facade.resolveChannel(undefined)).toEqual(Channel.LOCAL);
      expect(facade.instanceIdValue()).toBe("kep1");
    });

    it("an unrecognized config channel falls through to local", async () => {
      const { facade, messaging } = makeFacade({
        raw: { component: { instances: [{ id: "kep1", publish: { channel: "bogus" } }] } },
      });
      await facade.publish("temp", 1.0);
      expect(messaging.published[0].qos, "an unparseable channel -> LOCAL").toBeUndefined();
    });
  });

  describe("transport-failure isolation (readiness stays local)", () => {
    it("a northbound transport failure is swallowed", async () => {
      const messaging = new RecordingMessagingService();
      messaging.publishToIoTCore = vi.fn(async () => {
        throw new Error("iot core down");
      });
      const { facade } = makeFacade({ messaging });
      // A northbound outage must NOT propagate (it would otherwise flip local readiness).
      await expect(facade.signal("temp").addSample(1.0).via(Channel.NORTHBOUND).publish()).resolves.toBeUndefined();
    });

    it("a stream append failure is swallowed", async () => {
      const throwing: StreamSink = () => {
        throw new Error("stream buffer full");
      };
      const { facade, messaging } = makeFacade({ streamSink: throwing });
      // A stream-append outage must NOT propagate either.
      await expect(facade.signal("temp").addSample(1.0).via(Channel.stream("hot")).publish()).resolves.toBeUndefined();
      expect(messaging.published, "it tried the stream, not the bus").toHaveLength(0);
    });
  });
});
