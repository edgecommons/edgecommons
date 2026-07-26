/**
 * Every verb's happy path + each error code + the single-instance default; the allow-list refusal
 * proven to happen BEFORE any device I/O; pause gating a poll; and the panel registration. A mock
 * device loop services the control channel and RECORDS every write that reaches it — no device, no
 * socket.
 */
import { CommandInbox, Config, MetricService } from "@edgecommons/edgecommons";
import { afterEach, describe, expect, it } from "vitest";

import {
  DeviceConfig,
  DeviceControl,
  Health,
  Mailbox,
  Writes,
  setPaused,
} from "../src/app";
import { Commander, DeviceHandle, panels, registerAll } from "../src/commands";
import { BrowseError, Quality, SignalInfo } from "../src/device";
import { DeviceMetrics } from "../src/metrics";

// --- a no-op MetricService + Config so DeviceMetrics can be built without a live runtime ----------

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

function testConfig(): Config {
  return Config.fromValue("com.example.MyAdapter", "thing-1", {
    metricEmission: { target: "log", namespace: "test" },
    component: { global: {}, instances: [{ id: "plc-1" }] },
  });
}

function aDevice(): DeviceConfig {
  return {
    id: "plc-1",
    adapter: "sim",
    connection: { endpoint: "sim://plc-1" },
    pollIntervalMs: 5_000,
    writes: new Writes(["setpoint-1"]),
  };
}

function simSignals(): SignalInfo[] {
  return [
    { id: "temperature-1", name: "Ambient temperature" },
    { id: "setpoint-1", name: "Setpoint" },
  ];
}

function makeDm(cfg: DeviceConfig, health: Health): DeviceMetrics {
  return new DeviceMetrics(noopMetrics(), testConfig(), cfg.id, health, 30, simSignals().length);
}

type BrowseKind = "one" | "many" | "unsupported" | "failed";

interface MockOpts {
  writeOk: boolean;
  readOk: boolean;
  reconnectOk: boolean;
  repollOk: boolean;
  browse: BrowseKind;
}

const DEFAULT_OPTS: MockOpts = {
  writeOk: true,
  readOk: true,
  reconnectOk: true,
  repollOk: true,
  browse: "one",
};

interface Harness {
  commander: Commander;
  /** Every write that REACHED the device — empty proves the allow-list refused before any I/O. */
  writes: Array<[string, unknown]>;
  health: Health;
  stop: () => Promise<void>;
}

/** Build a single-device commander whose control channel is served by a mock device loop. */
function harness(cfg: DeviceConfig, opts: Partial<MockOpts> = {}): Harness {
  const o = { ...DEFAULT_OPTS, ...opts };
  const mailbox = new Mailbox<DeviceControl>();
  const health = new Health();
  health.setLink("ONLINE");
  const dm = makeDm(cfg, health);
  const writes: Array<[string, unknown]> = [];

  const loop = (async () => {
    for (;;) {
      const ctrl = await mailbox.receive(1_000);
      if (ctrl === undefined) {
        if (mailbox.isClosed()) return;
        continue;
      }
      service(ctrl, o, health, writes);
    }
  })();

  const handle: DeviceHandle = { cfg, control: mailbox, health, dm, signals: simSignals() };
  const commander = new Commander([handle]);
  const stop = async (): Promise<void> => {
    mailbox.close();
    await loop;
  };
  return { commander, writes, health, stop };
}

function service(ctrl: DeviceControl, o: MockOpts, health: Health, writes: Array<[string, unknown]>): void {
  switch (ctrl.kind) {
    case "write":
      writes.push([ctrl.signalId, ctrl.value]);
      ctrl.ack(o.writeOk ? { ok: true } : { ok: false, error: "device rejected" });
      break;
    case "readNow":
      if (o.readOk) {
        ctrl.reply({
          ok: true,
          readings: ctrl.ids.map((id) => ({
            signalId: id,
            value: 42,
            quality: Quality.Good,
            qualityRaw: "OK",
          })),
        });
      } else {
        ctrl.reply({ ok: false, error: "link error" });
      }
      break;
    case "browse":
      if (o.browse === "one") {
        ctrl.reply({
          ok: true,
          page: { entries: [{ id: "temperature-1", name: "Ambient temperature", typeName: "REAL" }] },
        });
      } else if (o.browse === "many") {
        ctrl.reply({
          ok: true,
          page: {
            entries: [
              { id: "temperature-1", name: "Ambient temperature", typeName: "REAL" },
              { id: "pressure-1", name: "Line pressure", typeName: "REAL" },
              { id: "setpoint-1", name: "Setpoint", typeName: "REAL" },
            ],
          },
        });
      } else if (o.browse === "unsupported") {
        ctrl.reply({ ok: false, error: BrowseError.unsupported() });
      } else {
        ctrl.reply({ ok: false, error: BrowseError.failed("mid-browse error") });
      }
      break;
    case "pause":
      ctrl.reply(setPaused(health, true));
      break;
    case "resume":
      ctrl.reply(setPaused(health, false));
      break;
    case "reconnect":
      ctrl.reply(o.reconnectOk ? { ok: true } : { ok: false, error: "no route to host" });
      break;
    case "repoll":
      ctrl.reply(o.repollOk ? { ok: true, polled: 2 } : { ok: false, error: "link error" });
      break;
  }
}

async function code(p: Promise<unknown>): Promise<string> {
  try {
    await p;
    throw new Error("expected the command to reject");
  } catch (e) {
    return (e as { code?: string }).code ?? "";
  }
}

let live: Harness | undefined;
afterEach(async () => {
  await live?.stop();
  live = undefined;
});

// --- routing / single-instance default (D-EIP-13) ---------------------------------------------

describe("instance routing (D-EIP-13)", () => {
  it("defaults to the sole device, and unknown or missing ids error", async () => {
    live = harness(aDevice());
    const out = await live.commander.status({});
    expect(out.id).toBe("plc-1");
    expect(await code(live.commander.status({ instance: "nope" }))).toBe("NO_SUCH_INSTANCE");

    // Two devices: a missing `instance` is BAD_ARGS. status() reads only health, so unstarted
    // control channels are fine here.
    const mk = (cfg: DeviceConfig): DeviceHandle => {
      const health = new Health();
      health.setLink("ONLINE");
      return { cfg, control: new Mailbox<DeviceControl>(), health, dm: makeDm(cfg, health), signals: simSignals() };
    };
    const b = { ...aDevice(), id: "plc-2" };
    const multi = new Commander([mk(aDevice()), mk(b)]);
    expect(await code(multi.status({}))).toBe("BAD_ARGS");
    expect((await multi.status({ instance: "plc-2" })).id).toBe("plc-2");
  });
});

// --- sb/status ---------------------------------------------------------------------------------

describe("sb/status", () => {
  it("reports connected, state, paused, and a counter snapshot", async () => {
    live = harness(aDevice());
    const out = await live.commander.status({});
    expect(out.connected).toBe(true);
    expect(out.state).toBe("ONLINE");
    expect(out.paused).toBe(false);
    expect(out.adapter).toBe("sim");
    expect((out.metrics as Record<string, unknown>).connectAttempts).toBeDefined();
  });
});

// --- sb/signals --------------------------------------------------------------------------------

describe("sb/signals", () => {
  it("lists the inventory with the writable flag", async () => {
    live = harness(aDevice());
    const out = await live.commander.signals({});
    const sigs = out.signals as Array<Record<string, unknown>>;
    expect(sigs).toHaveLength(2);
    expect(sigs.find((s) => s.id === "setpoint-1")?.writable).toBe(true); // on the allow-list
    expect(sigs.find((s) => s.id === "temperature-1")?.writable).toBe(false); // not
  });
});

// --- sb/read -----------------------------------------------------------------------------------

describe("sb/read", () => {
  it("returns values by id and by name and marks unresolved refs", async () => {
    live = harness(aDevice());
    const out = await live.commander.read({
      signals: [{ signalId: "temperature-1" }, { name: "Setpoint" }, { name: "ghost" }],
    });
    const reads = out.reads as Array<Record<string, unknown>>;
    expect((reads[0].signal as Record<string, unknown>).id).toBe("temperature-1");
    expect(reads[0].quality).toBe("GOOD");
    expect((reads[1].signal as Record<string, unknown>).id).toBe("setpoint-1"); // resolved by name
    expect(reads[2].quality).toBe("BAD"); // an unknown name is a BAD/unresolved entry
    expect(reads[2].qualityRaw).toBe("UNRESOLVED_REF");
  });

  it("is BAD_ARGS without a signals array, and READ_FAILED on a link error", async () => {
    live = harness(aDevice());
    expect(await code(live.commander.read({}))).toBe("BAD_ARGS");

    live = harness(aDevice(), { readOk: false });
    expect(await code(live.commander.read({ signals: [{ signalId: "temperature-1" }] }))).toBe("READ_FAILED");
  });
});

// --- sb/write: allow-list BEFORE any device I/O (the security guarantee) -----------------------

describe("sb/write", () => {
  it("never lets a refused write reach the device", async () => {
    live = harness(aDevice());
    // temperature-1 is NOT on the allow-list.
    expect(await code(live.commander.write({ writes: [{ signalId: "temperature-1", value: 1 }] }))).toBe(
      "WRITE_NOT_ALLOWED",
    );
    expect(live.writes).toHaveLength(0); // the refused write must never reach the device
    expect(live.health.writeErrors).toBe(0); // a policy refusal is not a device write error
  });

  it("confirms an allow-listed write and mixes batch results", async () => {
    live = harness(aDevice());
    // A single allowed write (single-object shorthand).
    let out = await live.commander.write({ signalId: "setpoint-1", value: 42 });
    expect(out.written).toBe(1);
    expect(live.writes).toHaveLength(1); // the allowed write reached the device

    // A batch: one allowed (written), one refused (never sent).
    out = await live.commander.write({
      writes: [
        { signalId: "setpoint-1", value: 7 },
        { signalId: "temperature-1", value: 8 },
      ],
    });
    expect(out.written).toBe(1); // only the allow-listed entry is written
    const results = out.results as Array<Record<string, unknown>>;
    expect(results.filter((r) => r.ok === true)).toHaveLength(1);
    expect(results.filter((r) => r.error === "not in writes.allow")).toHaveLength(1);
    // Two device writes total (one from each successful call); the refused entry added none.
    expect(live.writes).toHaveLength(2);
  });

  it("is WRITE_FAILED when the device rejects the write, and counts a write error", async () => {
    live = harness(aDevice(), { writeOk: false });
    expect(await code(live.commander.write({ signalId: "setpoint-1", value: 42 }))).toBe("WRITE_FAILED");
    expect(live.health.writeErrors).toBe(1); // the device rejection feeds the writeErrors counter
  });

  it("counts writeErrors only for device-path failures", async () => {
    // A successful write counts nothing; neither do caller errors (unresolved ref, missing value).
    live = harness(aDevice());
    await live.commander.write({ signalId: "setpoint-1", value: 42 });
    await live.commander.write({
      writes: [{ name: "ghost", value: 1 }, { signalId: "setpoint-1" }, { signalId: "setpoint-1", value: 2 }],
    });
    expect(live.health.writeErrors).toBe(0);
    await live.stop();

    // A gone device loop mid-write IS a device-path failure.
    const cfg = aDevice();
    const health = new Health();
    const control = new Mailbox<DeviceControl>();
    control.close();
    const handle: DeviceHandle = { cfg, control, health, dm: makeDm(cfg, health), signals: simSignals() };
    const commander = new Commander([handle]);
    expect(await code(commander.write({ signalId: "setpoint-1", value: 42 }))).toBe("DEVICE_UNAVAILABLE");
    expect(health.writeErrors).toBe(1);
  });

  it("is BAD_ARGS with no writes or value", async () => {
    live = harness(aDevice());
    expect(await code(live.commander.write({}))).toBe("BAD_ARGS");
  });
});

// --- sb/browse ---------------------------------------------------------------------------------

describe("sb/browse", () => {
  it("returns a page or the right error code", async () => {
    live = harness(aDevice());
    const out = await live.commander.browse({});
    const entries = out.entries as Array<Record<string, unknown>>;
    expect(entries).toHaveLength(1);
    expect(entries[0].id).toBe("temperature-1");
    await live.stop();

    live = harness(aDevice(), { browse: "unsupported" });
    expect(await code(live.commander.browse({}))).toBe("BROWSE_UNSUPPORTED");
    await live.stop();

    live = harness(aDevice(), { browse: "failed" });
    expect(await code(live.commander.browse({}))).toBe("BROWSE_FAILED");
  });
});

// --- sb/browse: the hierarchical panel mode ----------------------------------------------------

describe("sb/browse (hierarchical)", () => {
  it("answers the device node for ref root over the same inventory", async () => {
    live = harness(aDevice());
    const out = await live.commander.browse({ ref: "root" });

    expect(out.mode).toBe("hierarchical");
    expect(out.refCount).toBe(1);
    expect(out.depth).toBe(1);
    expect(out.truncated).toBe(false);

    const root = out.root as Record<string, unknown>;
    expect(root.nodeId).toBe("root");
    expect(root.name).toBe("plc-1"); // the root node is named from the instance
    expect(root.nodeClass).toBe("device");
    expect(root.dataType).toBeNull();

    const refs = root.refs as Array<Record<string, unknown>>;
    expect(refs[0].referenceType).toBe("contains");
    const target = refs[0].target as Record<string, unknown>;
    expect(target.nodeId).toBe("temperature-1");
    expect(target.name).toBe("Ambient temperature");
    expect(target.nodeClass).toBe("signal");
    expect(target.dataType).toBe("REAL");
  });

  it("answers a signal ref as a known leaf, and an unknown ref is BAD_ARGS", async () => {
    live = harness(aDevice());
    const out = await live.commander.browse({ ref: "temperature-1" });
    const root = out.root as Record<string, unknown>;
    expect(root.nodeId).toBe("temperature-1");
    expect(root.nodeClass).toBe("signal");
    expect(root.dataType).toBe("REAL");
    expect(root.refs).toEqual([]); // a known leaf answers refs: []
    expect(out.refCount).toBe(0);
    expect(out.truncated).toBe(false);

    expect(await code(live.commander.browse({ ref: "ghost" }))).toBe("BAD_ARGS");
  });

  it("refuses mixing hierarchical and paged args", async () => {
    live = harness(aDevice());
    expect(await code(live.commander.browse({ ref: "root", cursor: "x" }))).toBe("BAD_ARGS");
    expect(await code(live.commander.browse({ depth: 2, max: 10 }))).toBe("BAD_ARGS");
    // The hierarchical companions without `ref` are also refused: nothing to anchor the tree on.
    expect(await code(live.commander.browse({ depth: 2 }))).toBe("BAD_ARGS");
  });

  it("bounds depth and maxRefs and reports truncation", async () => {
    // depth clamps into 1..4; maxRefs into 1..1000.
    live = harness(aDevice());
    expect((await live.commander.browse({ ref: "root", depth: 99 })).depth).toBe(4);
    expect((await live.commander.browse({ ref: "root", depth: 0 })).depth).toBe(1);
    await live.stop();

    // Three browsable signals, maxRefs 2 -> two refs, truncated.
    live = harness(aDevice(), { browse: "many" });
    const out = await live.commander.browse({ ref: "root", maxRefs: 2 });
    expect(((out.root as Record<string, unknown>).refs as unknown[]).length).toBe(2);
    expect(out.refCount).toBe(2);
    expect(out.truncated).toBe(true);
  });

  it("maps the browse error codes like the paged mode", async () => {
    live = harness(aDevice(), { browse: "unsupported" });
    expect(await code(live.commander.browse({ ref: "root" }))).toBe("BROWSE_UNSUPPORTED");
    await live.stop();

    live = harness(aDevice(), { browse: "failed" });
    expect(await code(live.commander.browse({ ref: "root" }))).toBe("BROWSE_FAILED");
  });
});

// --- pause / resume / repoll -------------------------------------------------------------------

describe("pause / resume / repoll", () => {
  it("is idempotent and refuses repoll while paused", async () => {
    live = harness(aDevice());

    // repoll works while running.
    expect((await live.commander.repoll({})).polled).toBe(2);

    let out = await live.commander.pause({});
    expect(out.paused).toBe(true);
    expect(out.changed).toBe(true);
    expect(live.health.isPaused()).toBe(true);

    // repoll is refused while paused, with the dedicated PAUSED code.
    expect(await code(live.commander.repoll({}))).toBe("PAUSED");

    // pausing again is idempotent.
    expect((await live.commander.pause({})).changed).toBe(false);

    // resume clears it and repoll works again.
    out = await live.commander.resume({});
    expect(out.paused).toBe(false);
    expect(out.changed).toBe(true);
    expect(live.health.isPaused()).toBe(false);
    expect((await live.commander.repoll({})).polled).toBe(2);
  });
});

// --- reconnect ---------------------------------------------------------------------------------

describe("reconnect", () => {
  it("confirms or reports RECONNECT_FAILED", async () => {
    live = harness(aDevice());
    expect((await live.commander.reconnect({})).connected).toBe(true);
    await live.stop();

    live = harness(aDevice(), { reconnectOk: false });
    expect(await code(live.commander.reconnect({}))).toBe("RECONNECT_FAILED");
  });

  it("is DEVICE_UNAVAILABLE when the loop is gone", async () => {
    // A closed control channel stands in for a stopped device loop.
    const cfg = aDevice();
    const health = new Health();
    const control = new Mailbox<DeviceControl>();
    control.close();
    const handle: DeviceHandle = { cfg, control, health, dm: makeDm(cfg, health), signals: simSignals() };
    const commander = new Commander([handle]);
    expect(await code(commander.reconnect({}))).toBe("DEVICE_UNAVAILABLE");
  });
});

// --- panels ------------------------------------------------------------------------------------

describe("panels", () => {
  it("registers the three panels with the right ids, orders, and scope", () => {
    const ps = panels();
    expect(ps.map((p) => p.id)).toEqual(["overview", "signals", "diagnostics"]);
    expect(ps.map((p) => p.order)).toEqual([10, 20, 30]);
    for (const p of ps) expect(p.scope).toBe("instance");
    // The signals panel binds the signal verbs; diagnostics binds browse.
    expect(ps[1].verbs).toEqual(["sb/signals", "sb/read", "sb/write", "repoll"]);
    expect(ps[2].verbs).toEqual(["sb/browse", "sb/status"]);
  });

  it("emits exactly the renderable widget descriptors", () => {
    const ps = panels();

    // overview: a summary with rows + a lifecycle commandSummary with verbs.
    const overviewWidgets = ps[0].widgets as Array<Record<string, unknown>>;
    expect(overviewWidgets).toEqual([
      {
        kind: "summary",
        id: "overview-summary",
        title: "Adapter overview",
        rows: [
          { label: "Signals", value: "Configured signal inventory via cmd/sb/signals" },
          { label: "Lifecycle", value: "Pause, resume, reconnect, and repoll the instance" },
          { label: "Writes", value: "Allow-listed via writes.allow[]; checked before device I/O" },
        ],
      },
      {
        kind: "commandSummary",
        id: "overview-lifecycle",
        title: "Lifecycle bindings",
        verbs: ["sb/status", "reconnect", "sb/pause", "sb/resume", "repoll"],
      },
    ]);

    // signals: one signalGrid bound to sb/signals (with the subscriptionsVerb compat alias).
    expect(ps[1].widgets).toEqual([
      {
        kind: "signalGrid",
        id: "configured-signals",
        title: "Configured signals",
        scope: "instance",
        signalsVerb: "sb/signals",
        subscriptionsVerb: "sb/signals",
        readVerb: "sb/read",
      },
    ]);

    // diagnostics: the hierarchical treeBrowser + a diagnostic commandSummary.
    expect(ps[2].widgets).toEqual([
      {
        kind: "treeBrowser",
        id: "inventory-tree",
        title: "Inventory",
        scope: "instance",
        mode: "hierarchical",
        rootRef: "root",
        depth: 1,
        maxRefs: 200,
        browseVerb: "sb/browse",
        readVerb: "sb/read",
      },
      {
        kind: "commandSummary",
        id: "diagnostic-commands",
        title: "Diagnostic commands",
        verbs: ["sb/status", "sb/browse"],
      },
    ]);

    // No widget anywhere carries a writeVerb — the console has no write surface.
    for (const p of ps) {
      for (const w of p.widgets as Array<Record<string, unknown>>) {
        expect(w.writeVerb).toBeUndefined();
      }
    }
  });
});

describe("registerAll", () => {
  it("wires every sb/* verb and the three panels onto the command inbox", () => {
    // The runtime calls this once at startup. It must register the whole `sb/*` family (nothing
    // dropped, nothing renamed — those are wire contracts) plus the console panels.
    const registered: string[] = [];
    const registeredPanels: unknown[] = [];
    const inbox = {
      register: (name: string) => registered.push(name),
      registerPanel: (p: unknown) => registeredPanels.push(p),
    } as unknown as CommandInbox;

    const cfg = aDevice();
    const health = new Health();
    const handle: DeviceHandle = {
      cfg,
      control: new Mailbox<DeviceControl>(),
      health,
      dm: makeDm(cfg, health),
      signals: simSignals(),
    };

    registerAll(inbox, [handle]);

    expect(registered).toEqual([
      "sb/status",
      "sb/read",
      "sb/write",
      "sb/signals",
      "sb/browse",
      "sb/pause",
      "sb/resume",
      "reconnect",
      "repoll",
    ]);
    expect(registeredPanels).toHaveLength(3);
  });
});
