import { Config, MetricService } from "@edgecommons/edgecommons";
import { describe, expect, it } from "vitest";

import { Health } from "../src/app";
import {
  COMMAND,
  CONNECTION,
  DeviceMetrics,
  HEALTH,
  HEALTH_MEASURES,
  Pair,
  familyDefs,
} from "../src/metrics";

// --- a recording MetricService: captures every emitted (metric, values) pair --------------------

class RecordingMetrics implements MetricService {
  readonly emissions: Array<{ name: string; values: Record<string, number> }> = [];
  defineMetric(): void {}
  isMetricDefined(): boolean {
    return true;
  }
  async emitMetric(name: string, values: Record<string, number>): Promise<void> {
    this.emissions.push({ name, values });
  }
  async emitMetricNow(name: string, values: Record<string, number>): Promise<void> {
    this.emissions.push({ name, values });
  }
  async flushMetrics(): Promise<void> {}
  async shutdown(): Promise<void> {}
  last(name: string): Record<string, number> {
    return [...this.emissions].reverse().find((e) => e.name === name)?.values ?? {};
  }
}

function testConfig(): Config {
  return Config.fromValue("com.example.MyAdapter", "thing-1", {
    metricEmission: { target: "log", namespace: "test" },
    component: { global: {}, instances: [{ id: "plc-1" }] },
  });
}

const SECTION_5 = new Set([
  "connectionState",
  "publishLatencyMs",
  "pollLatencyMs",
  "readErrors",
  "staleSignals",
  "reconnects",
]);

describe("southbound_health parity (SOUTHBOUND.md §5)", () => {
  it("emits EXACTLY the §5 measure set — no more, no less", () => {
    const health = familyDefs().find((f) => f.name === HEALTH);
    expect(health).toBeDefined();
    const emitted = new Set(health!.measures.map((m) => m.name));
    expect(emitted).toEqual(SECTION_5);
    // The advertised const must agree with what familyDefs emits.
    expect(new Set(HEALTH_MEASURES)).toEqual(SECTION_5);
  });
});

describe("the operational-family pattern", () => {
  it("names the families from the component and keeps only low-cardinality dimensions", () => {
    const defs = familyDefs();
    const names = defs.map((f) => f.name);
    expect(names).toContain(CONNECTION);
    expect(names).toContain(COMMAND);
    // Named from the component token — a fleet view separates adapters by name.
    expect(CONNECTION.endsWith("Connection")).toBe(true);
    expect(CONNECTION).not.toBe("Connection");
    expect(COMMAND.endsWith("Command")).toBe(true);
    expect(COMMAND).not.toBe("Command");

    const cmd = defs.find((f) => f.name === COMMAND)!;
    expect(cmd.dimensions).toEqual(["instance", "verb", "result"]);
  });

  it("shapes the connection family as Total/Interval counter pairs plus the gauge and duration sum", () => {
    const conn = familyDefs().find((f) => f.name === CONNECTION)!;
    const names = conn.measures.map((m) => m.name);
    for (const base of ["connectAttempts", "connectFailures", "reconnectAttempts", "connectionDrops"]) {
      expect(names).toContain(`${base}Total`);
      expect(names).toContain(`${base}Interval`);
    }
    expect(names).toContain("connectionState"); // the state gauge
    expect(names).toContain("connectedDurationMs"); // the connected-duration sum
  });

  it("resets interval counters on drain but not totals", () => {
    const p = new Pair();
    p.add(3);
    const out: Record<string, number> = {};
    p.drainInto(out, "x");
    expect(out.xTotal).toBe(3);
    expect(out.xInterval).toBe(3);

    p.add(2);
    const out2: Record<string, number> = {};
    p.drainInto(out2, "x");
    expect(out2.xTotal).toBe(5); // total is monotonic across emits
    expect(out2.xInterval).toBe(2); // interval resets to only what accrued since the last emit
  });
});

describe("DeviceMetrics emission", () => {
  it("counts only signals past the staleSignalSecs threshold", async () => {
    const rec = new RecordingMetrics();
    const health = new Health();
    const dm = new DeviceMetrics(rec, testConfig(), "plc-1", health, 30);
    const now = Date.now();
    dm.onSignalUpdate("fresh", now);
    dm.onSignalUpdate("stale", now - 120_000);
    await dm.emitPeriodic();
    expect(rec.last(HEALTH).staleSignals).toBe(1);
  });

  it("drains the health interval counters on emit", async () => {
    const rec = new RecordingMetrics();
    const health = new Health();
    const dm = new DeviceMetrics(rec, testConfig(), "plc-1", health, 30);
    health.readErrors = 3;
    health.reconnects = 2;
    await dm.emitPeriodic();
    const h = rec.last(HEALTH);
    expect(h.readErrors).toBe(3);
    expect(h.reconnects).toBe(2);
    // Reset after the emit so the next interval starts clean.
    expect(health.readErrors).toBe(0);
    expect(health.reconnects).toBe(0);
  });

  it("records a command into the Command family's (verb, result) counters", async () => {
    const rec = new RecordingMetrics();
    const dm = new DeviceMetrics(rec, testConfig(), "plc-1", new Health(), 30);
    dm.recordCommand("sb/status", true, 5);
    await dm.emitPeriodic();
    // Exactly one (verb, result) combo saw a request; the rest drained zero.
    const withOneRequest = rec.emissions.filter(
      (e) => e.name === COMMAND && e.values.commandRequestsTotal === 1,
    );
    expect(withOneRequest).toHaveLength(1);
    expect(withOneRequest[0].values.commandErrorsTotal).toBe(0);
  });
});
