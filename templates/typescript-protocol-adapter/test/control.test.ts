/**
 * Unit coverage for the adapter's per-message control decisions — the functions the runtime seam
 * (`src/runtime.ts`) delegates to but which need no live runtime themselves: {@link pollOnce},
 * {@link handleControl}, {@link serveWhileDown}, and {@link buildDevices}. They are exercised here
 * against fake sessions/mailboxes; the loop that feeds them in production is validated on real infra
 * (`test/live-sim.test.ts`, the deploy paths).
 */
import { Config, DataFacade, EventsFacade, IMessagingService, Message, MetricService, Uns } from "@edgecommons/edgecommons";
import { describe, expect, it } from "vitest";

import {
  DeviceControl,
  Health,
  Mailbox,
  buildDevices,
  handleControl,
  pollOnce,
  readStaleSignalSecs,
  serveWhileDown,
} from "../src/app";
import { BrowseError, BrowsePage, DeviceSession, Quality, Reading, SimBackend } from "../src/device";
import { DeviceMetrics } from "../src/metrics";

// --- doubles -----------------------------------------------------------------------------------

function configFor(instances: Record<string, unknown>[]): Config {
  return Config.fromValue("com.example.Adapter", "gw-01", {
    hierarchy: { levels: ["site", "device"] },
    identity: { site: "factory-1" },
    component: { global: {}, instances },
  });
}

function dataFacadeFor(instanceId: string): { data: DataFacade; published: { topic: string; msg: Message }[] } {
  const config = configFor([{ id: instanceId }]);
  const published: { topic: string; msg: Message }[] = [];
  const messaging = {
    publish: async (topic: string, msg: Message): Promise<void> => {
      published.push({ topic, msg });
    },
  } as unknown as IMessagingService;
  const uns = new Uns(config.componentIdentity.withInstance(instanceId), config.topicIncludeRoot);
  return { data: new DataFacade(() => config, instanceId, uns, messaging, undefined), published };
}

/** A recording MetricService, enough to construct a real DeviceMetrics. */
class RecordingMetrics implements MetricService {
  defineMetric(): void {}
  isMetricDefined(): boolean {
    return true;
  }
  async emitMetric(): Promise<void> {}
  async emitMetricNow(): Promise<void> {}
  async flushMetrics(): Promise<void> {}
  async shutdown(): Promise<void> {}
}

function deviceMetricsFor(instanceId: string, health: Health): DeviceMetrics {
  return new DeviceMetrics(new RecordingMetrics(), configFor([{ id: instanceId }]), instanceId, health, 30);
}

/** An events double that records what was emitted; emit never rejects. */
function recorder(): { events: EventsFacade; emitted: { type: string }[] } {
  const emitted: { type: string }[] = [];
  const events = {
    emit: async (_severity: unknown, type?: string): Promise<void> => {
      emitted.push({ type: type ?? "" });
    },
    raiseAlarm: async (): Promise<void> => undefined,
    clearAlarm: async (): Promise<void> => undefined,
  } as unknown as EventsFacade;
  return { events, emitted };
}

/** A session whose methods can each be scripted to succeed or throw. */
function fakeSession(overrides: Partial<DeviceSession> = {}): DeviceSession & { closed: boolean } {
  const base = {
    closed: false,
    async readSignals(): Promise<Reading[]> {
      return [{ signalId: "s1", value: 1, quality: Quality.Good, qualityRaw: "OK" }];
    },
    async readNamed(ids: readonly string[]): Promise<Reading[]> {
      return ids.map((id) => ({ signalId: id, value: 1, quality: Quality.Good, qualityRaw: "OK" }));
    },
    async writeSignal(): Promise<void> {},
    async browse(): Promise<BrowsePage> {
      return { entries: [{ id: "s1", name: "S1", typeName: "num" }] };
    },
    async close(): Promise<void> {
      base.closed = true;
    },
  };
  return Object.assign(base, overrides) as DeviceSession & { closed: boolean };
}

const cfg = (id = "plc-1") => ({ id, adapter: "sim", connection: { endpoint: `sim://${id}` }, pollIntervalMs: 1000, writes: { permits: () => true } as never });

// --- pollOnce ----------------------------------------------------------------------------------

describe("pollOnce", () => {
  it("reads, publishes every reading, and records latencies", async () => {
    const { data, published } = dataFacadeFor("device-1");
    const health = new Health();
    const session = await new SimBackend().connect({ endpoint: "sim://device-1" });

    const r = await pollOnce(cfg("device-1"), session, data, deviceMetricsFor("device-1", health), health);

    expect(r).toEqual({ ok: true, polled: 2 });
    expect(published).toHaveLength(2);
  });

  it("reports a broken link — a read rejection is not a BAD sample, it is a lost connection", async () => {
    const { data } = dataFacadeFor("device-1");
    const health = new Health();
    const session = fakeSession({
      readSignals: async () => {
        throw new Error("connection reset");
      },
    });

    const r = await pollOnce(cfg(), session, data, deviceMetricsFor("device-1", health), health);

    expect(r).toEqual({ ok: false, polled: 0 });
    expect(health.readErrors).toBe(1);
  });
});

// --- handleControl -----------------------------------------------------------------------------

describe("handleControl during polling", () => {
  const setup = () => {
    const { data } = dataFacadeFor("device-1");
    const health = new Health();
    const { events } = recorder();
    return { data, health, events, dm: deviceMetricsFor("device-1", health) };
  };

  it("confirms a write and stays in the loop", async () => {
    const { data, health, events, dm } = setup();
    let acked: unknown;
    const exit = await handleControl(
      { kind: "write", signalId: "s1", value: 7, ack: (o) => (acked = o) },
      cfg(),
      fakeSession(),
      data,
      events,
      dm,
      health,
    );
    expect(exit).toBeUndefined();
    expect(acked).toEqual({ ok: true });
  });

  it("reports a failed write rather than pretending it landed", async () => {
    const { data, health, events, dm } = setup();
    let acked: { ok: boolean } | undefined;
    await handleControl(
      { kind: "write", signalId: "s1", value: 7, ack: (o) => (acked = o) },
      cfg(),
      fakeSession({ writeSignal: async () => { throw new Error("refused"); } }),
      data,
      events,
      dm,
      health,
    );
    expect(acked?.ok).toBe(false);
  });

  it("serves a live read (sb/read) and answers a read failure", async () => {
    const { data, health, events, dm } = setup();
    let ok: { ok: boolean; readings?: Reading[] } | undefined;
    await handleControl({ kind: "readNow", ids: ["s1"], reply: (o) => (ok = o) }, cfg(), fakeSession(), data, events, dm, health);
    expect(ok?.ok).toBe(true);
    expect(ok?.readings).toHaveLength(1);

    let bad: { ok: boolean } | undefined;
    await handleControl(
      { kind: "readNow", ids: ["s1"], reply: (o) => (bad = o) },
      cfg(),
      fakeSession({ readNamed: async () => { throw new Error("down"); } }),
      data,
      events,
      dm,
      health,
    );
    expect(bad?.ok).toBe(false);
  });

  it("pages a browse and normalizes a browse failure to a BrowseError", async () => {
    const { data, health, events, dm } = setup();
    let page: { ok: boolean } | undefined;
    await handleControl({ kind: "browse", max: 10, reply: (o) => (page = o) }, cfg(), fakeSession(), data, events, dm, health);
    expect(page?.ok).toBe(true);

    let fail: { ok: boolean; error?: BrowseError } | undefined;
    await handleControl(
      { kind: "browse", max: 10, reply: (o) => (fail = o) },
      cfg(),
      fakeSession({ browse: async () => { throw new Error("no browse"); } }),
      data,
      events,
      dm,
      health,
    );
    expect(fail?.ok).toBe(false);
    expect(BrowseError.isBrowseError(fail?.error)).toBe(true);
  });

  it("pauses and resumes idempotently, emitting only on an actual state change", async () => {
    const { data, health, dm } = setup();
    const { events, emitted } = recorder();
    let changed: boolean | undefined;
    await handleControl({ kind: "pause", reply: (c) => (changed = c) }, cfg(), fakeSession(), data, events, dm, health);
    expect(changed).toBe(true);
    expect(health.isPaused()).toBe(true);
    await handleControl({ kind: "pause", reply: (c) => (changed = c) }, cfg(), fakeSession(), data, events, dm, health);
    expect(changed).toBe(false); // already paused
    await handleControl({ kind: "resume", reply: (c) => (changed = c) }, cfg(), fakeSession(), data, events, dm, health);
    expect(changed).toBe(true);
    expect(emitted.map((e) => e.type)).toEqual(["adapter-paused", "adapter-resumed"]);
  });

  it("drops the session and asks the supervisor to reconnect", async () => {
    const { data, health, events, dm } = setup();
    const session = fakeSession();
    const exit = await handleControl({ kind: "reconnect", reply: () => undefined }, cfg(), session, data, events, dm, health);
    expect(exit).toEqual({ kind: "reconnect", reply: expect.any(Function) });
    expect(session.closed).toBe(true);
  });

  it("forces a repoll, refuses one while paused, and reports a link error", async () => {
    const { data, health, events, dm } = setup();
    let ok: { ok: boolean; polled?: number } | undefined;
    await handleControl({ kind: "repoll", reply: (o) => (ok = o) }, cfg(), fakeSession(), data, events, dm, health);
    expect(ok).toEqual({ ok: true, polled: 1 });

    health.paused = true;
    let refused: { ok: boolean; error?: string } | undefined;
    const noExit = await handleControl({ kind: "repoll", reply: (o) => (refused = o) }, cfg(), fakeSession(), data, events, dm, health);
    expect(noExit).toBeUndefined();
    expect(refused?.ok).toBe(false);
    expect(refused?.error).toMatch(/paused/);

    health.paused = false;
    let linkErr: { ok: boolean } | undefined;
    const exit = await handleControl(
      { kind: "repoll", reply: (o) => (linkErr = o) },
      cfg(),
      fakeSession({ readSignals: async () => { throw new Error("down"); } }),
      data,
      events,
      dm,
      health,
    );
    expect(linkErr?.ok).toBe(false);
    expect(exit).toEqual({ kind: "linkLost" });
  });
});

// --- serveWhileDown ----------------------------------------------------------------------------

describe("serveWhileDown (session is down)", () => {
  it("returns `elapsed` when the backoff window expires with no command", async () => {
    const { events } = recorder();
    const outcome = await serveWhileDown(new Mailbox<DeviceControl>(), events, new Health(), 10);
    expect(outcome).toEqual({ kind: "elapsed" });
  });

  it("returns `closed` when the control channel is closed (the loop is gone)", async () => {
    const { events } = recorder();
    const mailbox = new Mailbox<DeviceControl>();
    const pending = serveWhileDown(mailbox, events, new Health(), 60_000);
    mailbox.close();
    expect(await pending).toEqual({ kind: "closed" });
  });

  it("returns immediately on a reconnect request", async () => {
    const { events } = recorder();
    const mailbox = new Mailbox<DeviceControl>();
    mailbox.send({ kind: "reconnect", reply: () => undefined });
    expect(await serveWhileDown(mailbox, events, new Health(), 60_000)).toEqual({
      kind: "reconnect",
      reply: expect.any(Function),
    });
  });

  it("takes pause/resume while down, and answers every I/O verb `disconnected`", async () => {
    const { events } = recorder();
    const health = new Health();

    // pause takes effect immediately (only needs the shared flag), then the window elapses.
    const mbPause = new Mailbox<DeviceControl>();
    let paused: boolean | undefined;
    mbPause.send({ kind: "pause", reply: (c) => (paused = c) });
    expect(await serveWhileDown(mbPause, events, health, 5)).toEqual({ kind: "elapsed" });
    expect(paused).toBe(true);
    expect(health.isPaused()).toBe(true);

    // resume takes effect the same way.
    const mbResume = new Mailbox<DeviceControl>();
    let resumed: boolean | undefined;
    mbResume.send({ kind: "resume", reply: (c) => (resumed = c) });
    expect(await serveWhileDown(mbResume, events, health, 5)).toEqual({ kind: "elapsed" });
    expect(resumed).toBe(true);
    expect(health.isPaused()).toBe(false);

    // the I/O verbs are refused with "disconnected".
    for (const make of [
      (reply: (o: unknown) => void): DeviceControl => ({ kind: "write", signalId: "s", value: 1, ack: reply }),
      (reply: (o: unknown) => void): DeviceControl => ({ kind: "readNow", ids: ["s"], reply }),
      (reply: (o: unknown) => void): DeviceControl => ({ kind: "repoll", reply }),
      (reply: (o: unknown) => void): DeviceControl => ({ kind: "browse", max: 5, reply }),
    ]) {
      const mb = new Mailbox<DeviceControl>();
      let answer: { ok?: boolean; error?: unknown } | undefined;
      mb.send(make((o) => (answer = o as { ok?: boolean; error?: unknown })));
      expect(await serveWhileDown(mb, events, new Health(), 5)).toEqual({ kind: "elapsed" });
      expect(answer?.ok).toBe(false);
    }
  });
});

// --- buildDevices ------------------------------------------------------------------------------

describe("buildDevices", () => {
  it("skips a malformed device but keeps the valid ones", () => {
    const config = configFor([
      { id: "good-1", connection: { endpoint: "sim://good-1" } },
      { id: "bad-1", connection: {} }, // no endpoint → parseDevice throws
    ]);
    const devices = buildDevices(config);
    expect(devices.map((d) => d.id)).toEqual(["good-1"]);
  });

  it("fails loudly when no device is valid — idling silently is worse", () => {
    const config = configFor([{ id: "bad-1", connection: {} }]);
    expect(() => buildDevices(config)).toThrow(/no valid devices/);
  });
});

// --- readStaleSignalSecs -----------------------------------------------------------------------

describe("readStaleSignalSecs", () => {
  const withGlobal = (global: unknown): Config =>
    Config.fromValue("com.example.Adapter", "gw-01", { component: { global, instances: [] } });

  it("reads a configured `component.global.healthThresholds.staleSignalSecs`", () => {
    expect(readStaleSignalSecs(withGlobal({ healthThresholds: { staleSignalSecs: 45 } }))).toBe(45);
  });

  it("defaults to 30 when the threshold is absent or not a positive number", () => {
    expect(readStaleSignalSecs(withGlobal({}))).toBe(30);
    expect(readStaleSignalSecs(withGlobal({ healthThresholds: { staleSignalSecs: 0 } }))).toBe(30);
    expect(readStaleSignalSecs(withGlobal({ healthThresholds: "nope" }))).toBe(30);
  });
});
