import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";

import { Config } from "../src/config/model";
import { Heartbeat, HeartbeatMonitor } from "../src/heartbeat";
import { InstanceConnectivity } from "../src/instance_connectivity";
import type { MetricService, MeasureValues } from "../src/metrics/types";
import { RecordingMessagingService } from "./_fakes";

/** A MetricService that records emitMetricNow / defineMetric calls. */
class RecordingMetricService implements MetricService {
  readonly defined: string[] = [];
  readonly emittedNow: Array<{ name: string; values: MeasureValues }> = [];
  defineMetric(metric: { getName(): string }): void {
    this.defined.push(metric.getName());
  }
  isMetricDefined(): boolean {
    return true;
  }
  async emitMetric(): Promise<void> {}
  async emitMetricNow(name: string, values: MeasureValues): Promise<void> {
    this.emittedNow.push({ name, values });
  }
  async flushMetrics(): Promise<void> {}
  async shutdown(): Promise<void> {}
}

function cfg(heartbeat: Record<string, unknown>): Config {
  return Config.fromValue("com.example.C", "thing-1", { heartbeat });
}

/** The component's UNS state topic for the test identity (thing-1 / C, rootless). */
const STATE_TOPIC = "ecv1/thing-1/C/main/state";

beforeEach(() => {
  vi.useFakeTimers();
});
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe("HeartbeatMonitor extra coverage", () => {
  it("CPU first sample is 0 then a number on the second", () => {
    const m = new HeartbeatMonitor({ cpu: true, memory: false, disk: false, threads: false, files: false, fds: false });
    expect((m.getStats().cpu as Record<string, number>).cpu_usage).toBe(0);
    // burn some CPU between samples
    let x = 0;
    for (let i = 0; i < 1e6; i++) x += i;
    void x;
    expect(typeof (m.getStats().cpu as Record<string, number>).cpu_usage).toBe("number");
  });

  it("disabled measures are omitted; disk has the nested shape when enabled", () => {
    const m = new HeartbeatMonitor({ cpu: false, memory: false, disk: true, threads: true, files: true, fds: true });
    const s = m.getStats();
    expect(s).not.toHaveProperty("cpu");
    expect(s).not.toHaveProperty("memory");
    expect(s.disk).toHaveProperty("disk_total");
    expect(s.disk).toHaveProperty("disk_used");
    expect(s.disk).toHaveProperty("disk_free");
    expect(s.threads).toHaveProperty("threads");
    expect(s.fds).toHaveProperty("fds");
  });
});

describe("Heartbeat.start (UNS state keepalive + sys metric, §4.3)", () => {
  it("each tick publishes the state keepalive through the reserved seam AND emits the 'sys' metric", async () => {
    const metrics = new RecordingMetricService();
    const svc = new RecordingMessagingService();
    const config = cfg({ intervalSecs: 5, measures: { memory: true } });
    const hb = Heartbeat.start(() => config, metrics, svc);
    // start() fires the first tick immediately (async). Let microtasks run.
    await vi.advanceTimersByTimeAsync(0);

    // The measures metric is now named 'sys' (D-U20/D6).
    expect(metrics.defined).toContain("sys");
    expect(metrics.emittedNow.length).toBeGreaterThanOrEqual(1);
    expect(metrics.emittedNow[0].name).toBe("sys");
    expect(typeof metrics.emittedNow[0].values.memory_usage).toBe("number");

    // The state keepalive rides the privileged reserved seam (the state class is reserved).
    expect(svc.published.length).toBeGreaterThanOrEqual(1);
    const rec = svc.published[0];
    expect(rec.kind).toBe("publishReserved");
    expect(rec.topic).toBe(STATE_TOPIC);
    expect(rec.message!.header.name).toBe("state");
    expect(rec.message!.header.version).toBe("1.0");
    const body = rec.message!.getBody() as Record<string, unknown>;
    expect(body.status).toBe("RUNNING");
    expect(typeof body.uptimeSecs).toBe("number");
    // The envelope carries the component identity (single stamping site, §1.4).
    expect(rec.message!.getIdentity()?.device).toBe("thing-1");
    expect(rec.message!.getIdentity()?.component).toBe("C");
    expect(rec.message!.getIdentity()?.instance).toBe("main");
    await hb.stop();
  });

  it("destination iotcore routes the keepalive to IoT Core (measures unaffected)", async () => {
    const metrics = new RecordingMetricService();
    const svc = new RecordingMessagingService();
    const config = cfg({ measures: { memory: true }, destination: "iotcore" });
    const hb = Heartbeat.start(() => config, metrics, svc);
    await vi.advanceTimersByTimeAsync(0);
    expect(svc.published[0].kind).toBe("publishReservedToIoTCore");
    expect(svc.published[0].topic).toBe(STATE_TOPIC);
    expect(metrics.emittedNow[0].name).toBe("sys");
    await hb.stop();
  });

  it("with NO messaging service the sys metric still emits (no throw)", async () => {
    const metrics = new RecordingMetricService();
    const config = cfg({ measures: { memory: true } });
    const hb = Heartbeat.start(() => config, metrics, undefined);
    await vi.advanceTimersByTimeAsync(0);
    expect(metrics.defined).toContain("sys");
    expect(metrics.emittedNow[0].name).toBe("sys");
    await hb.stop();
  });

  it("heartbeat.enabled=false publishes nothing (and no STOPPED on stop)", async () => {
    const metrics = new RecordingMetricService();
    const svc = new RecordingMessagingService();
    const config = cfg({ enabled: false, measures: { memory: true } });
    const hb = Heartbeat.start(() => config, metrics, svc);
    await vi.advanceTimersByTimeAsync(0);
    expect(svc.published).toHaveLength(0);
    expect(metrics.emittedNow).toHaveLength(0);
    await hb.stop();
    expect(svc.published).toHaveLength(0);
  });

  it("a keepalive failure does not suppress the sys metric (each half best-effort)", async () => {
    const metrics = new RecordingMetricService();
    const svc = new RecordingMessagingService();
    svc.publishReserved = async () => {
      throw new Error("broker down");
    };
    const config = cfg({ measures: { memory: true } });
    const hb = Heartbeat.start(() => config, metrics, svc);
    await vi.advanceTimersByTimeAsync(0);
    expect(metrics.emittedNow.length).toBeGreaterThanOrEqual(1);
    expect(metrics.emittedNow[0].name).toBe("sys");
    await hb.stop();
  });

  it("an interval change rebuilds the timer cadence", async () => {
    const metrics = new RecordingMetricService();
    let config = cfg({ intervalSecs: 5, measures: { memory: true } });
    const hb = Heartbeat.start(() => config, metrics);
    await vi.advanceTimersByTimeAsync(0); // first immediate tick
    const afterStart = metrics.emittedNow.length;

    // Advance one 5s period -> one more tick. That tick sees the new interval (1s)
    // and rebuilds the timer.
    config = cfg({ intervalSecs: 1, measures: { memory: true } });
    await vi.advanceTimersByTimeAsync(5000);
    const afterInterval = metrics.emittedNow.length;
    expect(afterInterval).toBeGreaterThan(afterStart);

    // Now ticks should fire every 1s.
    await vi.advanceTimersByTimeAsync(3000);
    expect(metrics.emittedNow.length).toBeGreaterThan(afterInterval + 1);
    await hb.stop();
  });

  it("stop() halts ticks and publishes the best-effort STOPPED state exactly once", async () => {
    const metrics = new RecordingMetricService();
    const svc = new RecordingMessagingService();
    const config = cfg({ intervalSecs: 1, measures: { memory: true } });
    const hb = Heartbeat.start(() => config, metrics, svc);
    await vi.advanceTimersByTimeAsync(0);
    await hb.stop();
    const stopped = svc.published.filter(
      (r) => (r.message?.getBody() as Record<string, unknown> | undefined)?.status === "STOPPED",
    );
    expect(stopped).toHaveLength(1);
    expect(stopped[0].topic).toBe(STATE_TOPIC);
    // STOPPED body carries no uptimeSecs (the golden-envelope contract).
    expect("uptimeSecs" in (stopped[0].message!.getBody() as object)).toBe(false);

    // Idempotent: a second stop publishes nothing more and ticks stay halted.
    const count = svc.published.length;
    await hb.stop();
    await vi.advanceTimersByTimeAsync(5000);
    expect(svc.published.length).toBe(count);
    expect(metrics.emittedNow.length).toBeGreaterThanOrEqual(1);
  });

  it("a STOPPED publish failure is swallowed (shutdown proceeds)", async () => {
    const metrics = new RecordingMetricService();
    const svc = new RecordingMessagingService();
    const config = cfg({ intervalSecs: 1, measures: { memory: true } });
    const hb = Heartbeat.start(() => config, metrics, svc);
    await vi.advanceTimersByTimeAsync(0);
    svc.publishReserved = async () => {
      throw new Error("transport already closed");
    };
    await expect(hb.stop()).resolves.toBeUndefined();
  });
});

describe("Heartbeat per-instance connectivity (#1c)", () => {
  const lastBody = (svc: RecordingMessagingService): Record<string, unknown> =>
    svc.published.at(-1)!.message!.getBody() as Record<string, unknown>;

  it("the RUNNING keepalive carries the provider's instances[]; absent/empty/cleared omit it", async () => {
    const metrics = new RecordingMetricService();
    const svc = new RecordingMessagingService();
    const config = cfg({ intervalSecs: 60, measures: { memory: true } });
    const hb = Heartbeat.start(() => config, metrics, svc);
    await vi.advanceTimersByTimeAsync(0); // startup tick — no provider
    expect(lastBody(svc).instances).toBeUndefined();

    hb.setInstanceConnectivityProvider(() => [
      InstanceConnectivity.of("filler1", true, "opc.tcp://kep:49320"),
      InstanceConnectivity.of("kep2", false),
    ]);
    await hb.publishStateNow();
    const body = lastBody(svc);
    expect(body.status).toBe("RUNNING");
    expect(body.instances).toEqual([
      { instance: "filler1", connected: true, detail: "opc.tcp://kep:49320" },
      { instance: "kep2", connected: false },
    ]);

    hb.setInstanceConnectivityProvider(() => []); // empty -> omitted
    await hb.publishStateNow();
    expect(lastBody(svc).instances).toBeUndefined();

    hb.setInstanceConnectivityProvider(undefined); // cleared -> omitted
    await hb.publishStateNow();
    expect(lastBody(svc).instances).toBeUndefined();

    await hb.stop();
  });

  it("a throwing provider never suppresses the keepalive", async () => {
    const metrics = new RecordingMetricService();
    const svc = new RecordingMessagingService();
    const config = cfg({ intervalSecs: 60, measures: { memory: true } });
    const hb = Heartbeat.start(() => config, metrics, svc);
    await vi.advanceTimersByTimeAsync(0);
    hb.setInstanceConnectivityProvider(() => {
      throw new Error("boom");
    });
    await hb.publishStateNow();
    const body = lastBody(svc);
    expect(body.status).toBe("RUNNING");
    expect(body.instances).toBeUndefined();
    await hb.stop();
  });

  it("InstanceConnectivity serializes and validates", () => {
    expect(InstanceConnectivity.of("plc1", true, "tcp://10.0.0.50:502").toJson()).toEqual({
      instance: "plc1",
      connected: true,
      detail: "tcp://10.0.0.50:502",
    });
    expect(InstanceConnectivity.of("plc1", false).toJson()).toEqual({ instance: "plc1", connected: false });
    expect(new InstanceConnectivity("plc1", false, "  ").toJson().detail).toBeUndefined();
    expect(() => new InstanceConnectivity("", true)).toThrow();
    expect(() => new InstanceConnectivity("  ", true)).toThrow();
  });
});
