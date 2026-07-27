/**
 * The `sb/*` surface driven END TO END through a REAL `CommandInbox` — the delivery topic, the
 * envelope, the library's scope enforcement, the handler, and the reply — over a recording
 * messaging seam (no broker, no device).
 *
 * `commands.test.ts` calls the `Commander` directly, so it can only prove what this component does
 * with an *already resolved* instance. These tests prove the half the library now owns: two devices
 * are configured (so nothing resolves by default) and the command still reaches the right one
 * because the **delivery topic** addressed it. They also pin the two rules the adapter must NOT
 * re-implement — a body conflicting with the topic is refused before dispatch (the handler never
 * runs), and every verb advertises `"scope": "instance"` in `describe`.
 */
import { CommandInbox, Config, IMessagingService, Message, MessageBuilder, MetricService } from "@edgecommons/edgecommons";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { DeviceConfig, DeviceControl, Health, Mailbox, Writes, setPaused } from "../src/app";
import { DeviceHandle, registerAll } from "../src/commands";
import { SignalInfo } from "../src/device";
import { DeviceMetrics } from "../src/metrics";

const COMPONENT_FILTER = "ecv1/thing-1/MyAdapter/cmd/#";
const INSTANCE_FILTER = "ecv1/thing-1/MyAdapter/+/cmd/#";
const REPLY_TO = "edgecommons/reply-scoped-test";

/** The nine verbs this template registers, all of them instance-scoped. */
const SB_VERBS = [
  "sb/status",
  "sb/read",
  "sb/write",
  "sb/signals",
  "sb/browse",
  "sb/pause",
  "sb/resume",
  "reconnect",
  "repoll",
];

function noopMetrics(): MetricService {
  return {
    defineMetric: () => undefined,
    isMetricDefined: () => true,
    emitMetric: async () => undefined,
    emitMetricNow: async () => undefined,
    flushMetrics: async () => undefined,
    shutdown: async () => undefined,
  };
}

/** Two configured devices: nothing resolves by default, so the addressing has to carry the id. */
function testConfig(): Config {
  return Config.fromValue("com.example.MyAdapter", "thing-1", {
    metricEmission: { target: "log", namespace: "test" },
    component: { global: {}, instances: [{ id: "plc-1" }, { id: "plc-2" }] },
  });
}

function aDevice(id: string): DeviceConfig {
  return {
    id,
    adapter: "sim",
    connection: { endpoint: `sim://${id}` },
    pollIntervalMs: 5_000,
    writes: new Writes(["setpoint-1"]),
  };
}

const simSignals = (): SignalInfo[] => [{ id: "temperature-1", name: "Ambient temperature" }];

/** Records the inbox's subscriptions and its replies instead of talking to a broker. */
class RecordingMessaging {
  readonly subscriptions = new Map<string, (topic: string, message: Message) => void | Promise<void>>();
  readonly replies: Message[] = [];

  async subscribe(filter: string, handler: (topic: string, message: Message) => void | Promise<void>): Promise<void> {
    this.subscriptions.set(filter, handler);
  }

  async unsubscribe(filter: string): Promise<void> {
    this.subscriptions.delete(filter);
  }

  async reply(_request: Message, reply: Message): Promise<void> {
    this.replies.push(reply);
  }
}

/** One device plus the mock loop that services its control channel. */
interface Device {
  handle: DeviceHandle;
  health: Health;
  /** Every control message the device loop actually received — empty proves no handler ran. */
  serviced: string[];
  stop: () => Promise<void>;
}

function device(id: string): Device {
  const cfg = aDevice(id);
  const mailbox = new Mailbox<DeviceControl>();
  const health = new Health();
  health.setLink("ONLINE");
  const serviced: string[] = [];

  const loop = (async () => {
    for (;;) {
      const ctrl = await mailbox.receive(1_000);
      if (ctrl === undefined) {
        if (mailbox.isClosed()) return;
        continue;
      }
      serviced.push(ctrl.kind);
      switch (ctrl.kind) {
        case "pause":
          ctrl.reply(setPaused(health, true));
          break;
        case "resume":
          ctrl.reply(setPaused(health, false));
          break;
        case "reconnect":
          ctrl.reply({ ok: true });
          break;
        case "repoll":
          ctrl.reply({ ok: true, polled: 1 });
          break;
        case "readNow":
          ctrl.reply({ ok: true, readings: [] });
          break;
        case "browse":
          ctrl.reply({ ok: true, page: { entries: [] } });
          break;
        case "write":
          ctrl.ack({ ok: true });
          break;
      }
    }
  })();

  const handle: DeviceHandle = {
    cfg,
    control: mailbox,
    health,
    dm: new DeviceMetrics(noopMetrics(), testConfig(), id, health, 30, simSignals().length),
    signals: simSignals(),
  };
  return {
    handle,
    health,
    serviced,
    stop: async () => {
      mailbox.close();
      await loop;
    },
  };
}

describe("scoped command delivery", () => {
  let messaging: RecordingMessaging;
  let inbox: CommandInbox;
  let devices: Record<string, Device>;

  beforeEach(async () => {
    devices = { "plc-1": device("plc-1"), "plc-2": device("plc-2") };
    messaging = new RecordingMessaging();
    inbox = new CommandInbox(
      testConfig,
      messaging as unknown as IMessagingService,
      () => 42,
      () => true,
      () => ({}),
    );
    registerAll(inbox, [devices["plc-1"].handle, devices["plc-2"].handle]);
    await inbox.start();
    expect(new Set(messaging.subscriptions.keys())).toEqual(new Set([COMPONENT_FILTER, INSTANCE_FILTER]));
  });

  afterEach(async () => {
    await inbox.close();
    await devices["plc-1"].stop();
    await devices["plc-2"].stop();
  });

  /** Delivers one request on `topic` and returns the single reply body. */
  async function deliver(topic: string, verb: string, body: Record<string, unknown>): Promise<Record<string, unknown>> {
    messaging.replies.length = 0;
    const request = MessageBuilder.create(verb, "1.0").withPayload(body).withReplyTo(REPLY_TO).build();
    const filter = topic.startsWith("ecv1/thing-1/MyAdapter/cmd/") ? COMPONENT_FILTER : INSTANCE_FILTER;
    await messaging.subscriptions.get(filter)!(topic, request);
    expect(messaging.replies, `exactly one reply expected for ${topic}`).toHaveLength(1);
    return messaging.replies[0].getBody() as Record<string, unknown>;
  }

  function okResult(reply: Record<string, unknown>): Record<string, unknown> {
    expect(reply.ok, `expected a success reply, got ${JSON.stringify(reply)}`).toBe(true);
    return reply.result as Record<string, unknown>;
  }

  function error(reply: Record<string, unknown>): { code: string; message: string } {
    expect(reply.ok, `expected an error reply, got ${JSON.stringify(reply)}`).toBe(false);
    return reply.error as { code: string; message: string };
  }

  // --- the addressing the library resolves ------------------------------------------------------

  it("routes an instance-addressed topic with no instance in the body", async () => {
    const result = okResult(await deliver("ecv1/thing-1/MyAdapter/plc-2/cmd/sb/status", "sb/status", {}));
    // Two devices are configured, so nothing but the topic could have selected this one.
    expect(result.id).toBe("plc-2");
  });

  it("still honors an instance named in the body of a component-addressed command", async () => {
    const result = okResult(
      await deliver("ecv1/thing-1/MyAdapter/cmd/sb/status", "sb/status", { instance: "plc-1" }),
    );
    expect(result.id).toBe("plc-1");
  });

  it("is BAD_ARGS when a component-addressed command names no instance and two devices exist", async () => {
    const e = error(await deliver("ecv1/thing-1/MyAdapter/cmd/sb/status", "sb/status", {}));
    expect(e.code).toBe("BAD_ARGS");
    // The component-side default policy answers here — the library cannot know the config.
    expect(e.message).toBe("field `instance` is required when multiple devices are configured");
  });

  it("is NO_SUCH_INSTANCE for an addressed instance that is not configured", async () => {
    const e = error(await deliver("ecv1/thing-1/MyAdapter/ghost/cmd/sb/status", "sb/status", {}));
    expect(e.code).toBe("NO_SUCH_INSTANCE");
  });

  it("refuses a body that conflicts with the topic, and the handler never runs", async () => {
    const e = error(
      await deliver("ecv1/thing-1/MyAdapter/plc-1/cmd/sb/pause", "sb/pause", { instance: "plc-2" }),
    );
    expect(e.code).toBe("BAD_ARGS");
    expect(e.message).toBe("instance in body conflicts with the addressed instance");

    // The proof that the rejection happened BEFORE dispatch: neither device was paused, and neither
    // control loop was touched. This adapter has no conflict check of its own to run.
    expect(devices["plc-1"].health.isPaused()).toBe(false);
    expect(devices["plc-2"].health.isPaused()).toBe(false);
    expect(devices["plc-1"].serviced).toEqual([]);
    expect(devices["plc-2"].serviced).toEqual([]);
  });

  it("acts on the addressed device only", async () => {
    const result = okResult(await deliver("ecv1/thing-1/MyAdapter/plc-1/cmd/sb/pause", "sb/pause", {}));
    expect(result.id).toBe("plc-1");
    expect(result.paused).toBe(true);
    expect(devices["plc-1"].health.isPaused()).toBe(true);
    expect(devices["plc-2"].health.isPaused()).toBe(false); // the other device is untouched
  });

  // --- what describe advertises ------------------------------------------------------------------

  it("advertises every sb/* verb as instance-scoped in describe", async () => {
    const result = okResult(await deliver("ecv1/thing-1/MyAdapter/cmd/describe", "describe", {}));
    const commands = result.commands as Array<Record<string, unknown>>;
    const byVerb = new Map(commands.map((c) => [c.verb as string, c]));
    for (const verb of SB_VERBS) {
      const entry = byVerb.get(verb);
      expect(entry, `describe is missing the verb ${verb}`).toBeDefined();
      expect(entry!.scope, `${verb} is registered instance-scoped`).toBe("instance");
      expect(entry!.builtIn).toBe(false);
    }
    // The library's own verbs answer at either scope.
    expect(byVerb.get("ping")!.scope).toBe("both");
  });
});
