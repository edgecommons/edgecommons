import { Config, DataFacade, IMessagingService, Message, Quality as LibQuality, Uns } from "@edgecommons/edgecommons";
import { describe, expect, it } from "vitest";

import {
  Backoff,
  DeviceControl,
  Health,
  Mailbox,
  Writes,
  connectivityOf,
  parseDevice,
  publishReadings,
  setPaused,
  toLibQuality,
} from "../src/app";
import { Quality, SimBackend } from "../src/device";

// --- a config + data() facade wired to a recording transport ----------------------------------

interface Published {
  topic: string;
  msg: Message;
}

function dataFacadeFor(instanceId: string): { data: DataFacade; published: Published[] } {
  const config = Config.fromValue("com.example.Adapter", "gw-01", {
    hierarchy: { levels: ["site", "device"] },
    identity: { site: "factory-1" },
    component: { global: {}, instances: [{ id: instanceId }] },
  });
  const published: Published[] = [];
  const messaging = {
    publish: async (topic: string, msg: Message): Promise<void> => {
      published.push({ topic, msg });
    },
  } as unknown as IMessagingService;

  const uns = new Uns(config.componentIdentity.withInstance(instanceId), config.topicIncludeRoot);
  const data = new DataFacade(() => config, instanceId, uns, messaging, undefined);
  return { data, published };
}

describe("device config", () => {
  it("parses from its instance config", () => {
    const d = parseDevice({
      id: "plc-1",
      adapter: "sim",
      connection: { endpoint: "sim://plc-1", unitId: 3 },
      pollIntervalMs: 1000,
      writes: { allow: ["setpoint-1"] },
    });

    expect(d.id).toBe("plc-1");
    expect(d.pollIntervalMs).toBe(1000);
    // `connection` is deliberately open: every protocol needs different keys.
    expect(d.connection.unitId).toBe(3);
  });

  it("is read-only until a write is allow-listed", () => {
    // The default must be read-only. An adapter that writes any address it is asked to is a
    // control-system vulnerability, not a convenience.
    const d = parseDevice({ id: "plc-1", connection: { endpoint: "sim://plc-1" } });
    expect(d.writes.allow).toEqual([]);
    expect(d.writes.permits("setpoint-1")).toBe(false);

    const w = new Writes(["setpoint-1"]);
    expect(w.permits("setpoint-1")).toBe(true);
    expect(w.permits("setpoint-2")).toBe(false); // only the listed signal, not its neighbours
  });

  it("rejects an unknown config key rather than ignoring it", () => {
    // A typo'd key is a mistake, not a no-op.
    expect(() => parseDevice({ id: "plc-1", connection: { endpoint: "x" }, pollIntervalMS: 1000 })).toThrow(
      /unknown key/,
    );
  });

  it("rejects each malformed field with a specific message rather than a vague failure", () => {
    expect(() => parseDevice({ id: "plc-1" })).toThrow(/`connection` is required/);
    expect(() => parseDevice({ id: "plc-1", connection: { endpoint: "x" }, pollIntervalMs: 0 })).toThrow(
      /`pollIntervalMs` must be a positive number/,
    );
    expect(() =>
      parseDevice({ id: "plc-1", connection: { endpoint: "x" }, writes: { allow: [1, 2] } }),
    ).toThrow(/`writes.allow` must be an array of signal ids/);
  });
});

describe("quality mapping", () => {
  it("maps every backend quality onto the library's wire enum", () => {
    // A backend speaks protocol-free quality; the wire speaks the library enum. Every case must map.
    expect(toLibQuality(Quality.Good)).toBe(LibQuality.Good);
    expect(toLibQuality(Quality.Bad)).toBe(LibQuality.Bad);
    expect(toLibQuality(Quality.Uncertain)).toBe(LibQuality.Uncertain);
  });
});

describe("reconnect backoff", () => {
  it("is exponential, capped, and jittered", () => {
    const b = new Backoff(1_000, 10_000);
    expect(b.delayMs(0, 1.0)).toBe(1_000);
    expect(b.delayMs(2, 1.0)).toBe(4_000);
    expect(b.delayMs(20, 1.0)).toBe(10_000); // capped
    // Full jitter: the delay is a point in the window, not its edge — so a plant full of adapters
    // does not reconnect in lockstep when a PLC reboots.
    expect(b.delayMs(2, 0.5)).toBe(2_000);
    expect(b.delayMs(2, 0.0)).toBe(0);
  });
});

describe("per-instance connectivity", () => {
  const device = parseDevice({ id: "plc-1", adapter: "sim", connection: { endpoint: "sim://plc-1" } });

  it("reports a configured device that has not connected yet", () => {
    // The health exists from the moment the device is CONFIGURED. A configured device that is down
    // must never look like a device nobody configured — so it is reported, connected=false, and the
    // adapter's own token says WHY it is not up: CONNECTING is not BACKOFF.
    const c = connectivityOf(device, new Health());

    expect(c.instance).toBe("plc-1");
    expect(c.connected).toBe(false);
    expect(c.state).toBe("CONNECTING");
    expect(c.detail).toBe("sim://plc-1"); // the endpoint, for a human
    expect(c.attributes.adapter).toBe("sim"); // the open bag carries domain data
    expect(c.attributes.paused).toBe(false);
  });

  it("goes ONLINE on connect and BACKOFF on failure", () => {
    const health = new Health();

    health.setLink("ONLINE");
    expect(connectivityOf(device, health).connected).toBe(true); // the flag every console reads
    expect(connectivityOf(device, health).state).toBe("ONLINE");

    health.setLink("BACKOFF");
    expect(connectivityOf(device, health).connected).toBe(false);
    expect(connectivityOf(device, health).state).toBe("BACKOFF");
  });

  it("reports PAUSED while online-and-paused but stays connected", () => {
    const health = new Health();
    health.setLink("ONLINE");

    expect(setPaused(health, true)).toBe(true); // pausing changed the state
    expect(setPaused(health, true)).toBe(false); // pausing again is idempotent
    let c = connectivityOf(device, health);
    expect(c.state).toBe("PAUSED"); // paused + online = PAUSED
    expect(c.connected).toBe(true); // connected stays truthful while paused
    expect(c.attributes.paused).toBe(true);

    // A break while paused reports BACKOFF (not PAUSED), connected false.
    health.setLink("BACKOFF");
    c = connectivityOf(device, health);
    expect(c.state).toBe("BACKOFF");
    expect(c.connected).toBe(false);
  });

  it("keeps the normalized flag and the health metric from disagreeing", () => {
    // Both move through setLink, so the metric an operator charts and the connectivity a console
    // renders are the same fact.
    const health = new Health();
    health.setLink("ONLINE");
    expect(health.connectionState).toBe(1);
    health.setLink("BACKOFF");
    expect(health.connectionState).toBe(0);
  });
});

describe("the southbound publish path", () => {
  it("publishes every reading through data(), quality and all", async () => {
    const { data, published } = dataFacadeFor("device-1");
    const session = await new SimBackend().connect({ endpoint: "sim://device-1" });

    const count = await publishReadings(
      data,
      "sim",
      { id: "device-1", connection: { endpoint: "sim://device-1" } },
      await session.readSignals(),
    );

    expect(count).toBe(2);
    expect(published.map((p) => p.topic)).toEqual([
      "ecv1/gw-01/Adapter/device-1/data/temperature-1",
      "ecv1/gw-01/Adapter/device-1/data/pressure-1",
    ]);
  });

  it("publishes a BAD-quality read rather than swallowing it", async () => {
    // A signal that silently stops updating is indistinguishable from one that is not changing.
    const { data, published } = dataFacadeFor("device-1");

    await publishReadings(
      data,
      "sim",
      { id: "device-1", connection: { endpoint: "sim://device-1" } },
      [{ signalId: "pressure-1", name: "Line pressure", value: null, quality: Quality.Bad, qualityRaw: "SENSOR_FAULT" }],
    );

    expect(published).toHaveLength(1);
    const body = published[0].msg.body as {
      device: Record<string, unknown>;
      signal: Record<string, unknown>;
      samples: Record<string, unknown>[];
    };
    expect(published[0].topic).toBe("ecv1/gw-01/Adapter/device-1/data/pressure-1");
    expect(body.device).toEqual({ adapter: "sim", instance: "device-1", endpoint: "sim://device-1" });
    expect(body.signal.id).toBe("pressure-1");
    expect(body.samples[0].quality).toBe("BAD");
    expect(body.samples[0].qualityRaw).toBe("SENSOR_FAULT");
    // The envelope carries our identity — the facade stamped it; we never hand-built it.
    expect(published[0].msg.identity?.instance).toBe("device-1");
  });

  it("swallows a single failed publish and keeps going — one bad signal must not blind the rest", async () => {
    // A publish that rejects (transport hiccup) is logged and skipped, not thrown: the next reading
    // still gets its chance. The count returned is only the readings that actually went out.
    const config = Config.fromValue("com.example.Adapter", "gw-01", {
      hierarchy: { levels: ["site", "device"] },
      identity: { site: "factory-1" },
      component: { global: {}, instances: [{ id: "device-1" }] },
    });
    let calls = 0;
    const messaging = {
      publish: async (): Promise<void> => {
        calls += 1;
        if (calls === 1) throw new Error("transport hiccup");
      },
    } as unknown as IMessagingService;
    const uns = new Uns(config.componentIdentity.withInstance("device-1"), config.topicIncludeRoot);
    const data = new DataFacade(() => config, "device-1", uns, messaging, undefined);

    const count = await publishReadings(data, "sim", { id: "device-1", connection: { endpoint: "sim://device-1" } }, [
      { signalId: "s1", value: 1, quality: Quality.Good, qualityRaw: "OK" },
      { signalId: "s2", value: 2, quality: Quality.Good, qualityRaw: "OK" },
    ]);

    expect(calls).toBe(2); // both attempted
    expect(count).toBe(1); // only the second succeeded
  });
});

describe("the control mailbox", () => {
  it("hands a queued control message to the single consumer, and times out when there is none", async () => {
    const mailbox = new Mailbox<string>();
    mailbox.send("first");
    expect(await mailbox.receive(50)).toBe("first");
    expect(await mailbox.receive(10)).toBeUndefined();
  });

  it("refuses sends once closed (the device loop is gone → DEVICE_UNAVAILABLE)", async () => {
    const mailbox = new Mailbox<DeviceControl>();
    mailbox.close();
    expect(mailbox.send({ kind: "resume", reply: () => undefined })).toBe(false);
    // A pending receiver wakes on close and drains nothing.
    expect(await mailbox.receive(1_000)).toBeUndefined();
    expect(mailbox.isClosed()).toBe(true);
  });
});
