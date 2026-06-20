import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";

import { Config } from "../src/config/model";
import { Heartbeat, HeartbeatMonitor } from "../src/heartbeat";
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

describe("Heartbeat.start dispatch", () => {
  it("metric target -> emitMetricNow('heartbeat', flattened)", async () => {
    const metrics = new RecordingMetricService();
    const config = cfg({ intervalSecs: 5, measures: { memory: true }, targets: [{ type: "metric" }] });
    const hb = Heartbeat.start(() => config, metrics);
    // start() fires the first tick immediately (async). Let microtasks run.
    await vi.advanceTimersByTimeAsync(0);
    expect(metrics.defined).toContain("heartbeat");
    expect(metrics.emittedNow.length).toBeGreaterThanOrEqual(1);
    expect(metrics.emittedNow[0].name).toBe("heartbeat");
    expect(typeof metrics.emittedNow[0].values.memory_usage).toBe("number");
    hb.stop();
  });

  it("messaging target destination ipc -> publish with name 'heartbeat' v'1.0.0'", async () => {
    const metrics = new RecordingMetricService();
    const svc = new RecordingMessagingService();
    const config = cfg({
      intervalSecs: 5,
      measures: { memory: true },
      targets: [{ type: "messaging", config: { destination: "ipc", topic: "hb/{ThingName}/x" } }],
    });
    const hb = Heartbeat.start(() => config, metrics, svc);
    await vi.advanceTimersByTimeAsync(0);
    expect(svc.published.length).toBeGreaterThanOrEqual(1);
    const rec = svc.published[0];
    expect(rec.kind).toBe("publish");
    expect(rec.topic).toBe("hb/thing-1/x");
    expect(rec.message!.header.name).toBe("heartbeat");
    expect(rec.message!.header.version).toBe("1.0.0");
    hb.stop();
  });

  it("messaging target destination iot_core -> publishToIotCore", async () => {
    const metrics = new RecordingMetricService();
    const svc = new RecordingMessagingService();
    const config = cfg({
      measures: { memory: true },
      targets: [{ type: "messaging", config: { destination: "iot_core" } }],
    });
    const hb = Heartbeat.start(() => config, metrics, svc);
    await vi.advanceTimersByTimeAsync(0);
    expect(svc.published[0].kind).toBe("publishToIotCore");
    hb.stop();
  });

  it("messaging target with NO messaging service is skipped (no throw)", async () => {
    const metrics = new RecordingMetricService();
    const config = cfg({ measures: { memory: true }, targets: [{ type: "messaging", config: {} }] });
    const hb = Heartbeat.start(() => config, metrics, undefined);
    await vi.advanceTimersByTimeAsync(0);
    // No crash; the only assertion is that ticking did not throw and metric was defined.
    expect(metrics.defined).toContain("heartbeat");
    hb.stop();
  });

  it("unknown destination and unknown type are skipped", async () => {
    const metrics = new RecordingMetricService();
    const svc = new RecordingMessagingService();
    const config = cfg({
      measures: { memory: true },
      targets: [
        { type: "messaging", config: { destination: "carrier-pigeon" } },
        { type: "telepathy" },
      ],
    });
    const hb = Heartbeat.start(() => config, metrics, svc);
    await vi.advanceTimersByTimeAsync(0);
    expect(svc.published).toHaveLength(0);
    hb.stop();
  });

  it("an interval change rebuilds the timer cadence", async () => {
    const metrics = new RecordingMetricService();
    let config = cfg({ intervalSecs: 5, measures: { memory: true }, targets: [{ type: "metric" }] });
    const hb = Heartbeat.start(() => config, metrics);
    await vi.advanceTimersByTimeAsync(0); // first immediate tick
    const afterStart = metrics.emittedNow.length;

    // Advance one 5s period -> one more tick. That tick sees the new interval (1s)
    // and rebuilds the timer.
    config = cfg({ intervalSecs: 1, measures: { memory: true }, targets: [{ type: "metric" }] });
    await vi.advanceTimersByTimeAsync(5000);
    const afterInterval = metrics.emittedNow.length;
    expect(afterInterval).toBeGreaterThan(afterStart);

    // Now ticks should fire every 1s.
    await vi.advanceTimersByTimeAsync(3000);
    expect(metrics.emittedNow.length).toBeGreaterThan(afterInterval + 1);
    hb.stop();
  });

  it("stop() halts ticks", async () => {
    const metrics = new RecordingMetricService();
    const config = cfg({ intervalSecs: 1, measures: { memory: true }, targets: [{ type: "metric" }] });
    const hb = Heartbeat.start(() => config, metrics);
    await vi.advanceTimersByTimeAsync(0);
    hb.stop();
    const count = metrics.emittedNow.length;
    await vi.advanceTimersByTimeAsync(5000);
    expect(metrics.emittedNow.length).toBe(count);
  });
});
