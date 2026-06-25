/**
 * CredentialMetricsBridge tests — the bridge defines a `credentials` metric on the metric service at
 * construction and periodically emits the non-sensitive {@link CredentialStats} (never the value).
 * The metric service is mocked; timers are faked so the periodic `tick` is deterministic. Mirrors the
 * Rust `CredentialMetricsBridge` behavior.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { Config } from "../src/config/model";
import { CredentialMetricsBridge } from "../src/credentials/bridge";
import type { CredentialService } from "../src/credentials/service";
import type { CredentialStats } from "../src/credentials/types";
import type { Metric } from "../src/metrics/metric";
import type { MeasureValues, MetricService } from "../src/metrics/types";

/** A spying metric service capturing definitions + emitted values. */
class FakeMetricService implements MetricService {
  defined?: Metric;
  emitted: Array<[string, MeasureValues]> = [];
  emitError?: Error;

  defineMetric(metric: Metric): void {
    this.defined = metric;
  }
  isMetricDefined(name: string): boolean {
    return this.defined?.getName() === name;
  }
  async emitMetric(name: string, values: MeasureValues): Promise<void> {
    if (this.emitError) throw this.emitError;
    this.emitted.push([name, values]);
  }
  async emitMetricNow(name: string, values: MeasureValues): Promise<void> {
    return this.emitMetric(name, values);
  }
  async flushMetrics(): Promise<void> {}
  async shutdown(): Promise<void> {}
}

/** A stub credential service that only needs `stats()` for the bridge. */
function stubCreds(stats: CredentialStats, onStats?: () => void): CredentialService {
  return {
    stats() {
      onStats?.();
      return stats;
    },
  } as unknown as CredentialService;
}

function config(): Config {
  return Config.fromValue("com.example.Comp", "thing-1", {
    metricEmission: { target: "log", namespace: "ns" },
  });
}

beforeEach(() => {
  vi.useFakeTimers();
});
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe("CredentialMetricsBridge", () => {
  it("defines the `credentials` metric with all four measures at construction", () => {
    const metrics = new FakeMetricService();
    const bridge = new CredentialMetricsBridge(config(), metrics, stubCreds({ secretCount: 0, syncFailures: 0, rotations: 0 }));

    expect(metrics.defined).toBeDefined();
    expect(metrics.defined!.getName()).toBe("credentials");
    const measures = metrics.defined!.getMeasures();
    expect([...measures.keys()].sort()).toEqual(["lastSyncAgeMs", "rotations", "secretCount", "syncFailures"]);
    expect(measures.get("lastSyncAgeMs")!.unit).toBe("Milliseconds");
    expect(measures.get("secretCount")!.unit).toBe("Count");
    bridge.close();
  });

  it("uses high storage resolution (1) for sub-minute intervals", () => {
    const metrics = new FakeMetricService();
    const bridge = new CredentialMetricsBridge(config(), metrics, stubCreds({ secretCount: 0, syncFailures: 0, rotations: 0 }), 30);
    expect(metrics.defined!.getMeasure("secretCount")!.storageResolution).toBe(1);
    bridge.close();
  });

  it("uses standard resolution (60) for >=60s intervals", () => {
    const metrics = new FakeMetricService();
    const bridge = new CredentialMetricsBridge(config(), metrics, stubCreds({ secretCount: 0, syncFailures: 0, rotations: 0 }), 60);
    expect(metrics.defined!.getMeasure("secretCount")!.storageResolution).toBe(60);
    bridge.close();
  });

  it("emits the credential stats once per interval", async () => {
    const metrics = new FakeMetricService();
    const stats: CredentialStats = { secretCount: 4, lastSyncAgeMs: 1234, syncFailures: 2, rotations: 7 };
    const bridge = new CredentialMetricsBridge(config(), metrics, stubCreds(stats), 10);

    await vi.advanceTimersByTimeAsync(10_000);

    expect(metrics.emitted.length).toBe(1);
    const [name, values] = metrics.emitted[0];
    expect(name).toBe("credentials");
    expect(values).toEqual({ secretCount: 4, lastSyncAgeMs: 1234, syncFailures: 2, rotations: 7 });

    await vi.advanceTimersByTimeAsync(10_000);
    expect(metrics.emitted.length).toBe(2);
    bridge.close();
  });

  it("defaults lastSyncAgeMs to 0 when there is no central sync (undefined)", async () => {
    const metrics = new FakeMetricService();
    const stats: CredentialStats = { secretCount: 1, syncFailures: 0, rotations: 0 }; // lastSyncAgeMs undefined
    const bridge = new CredentialMetricsBridge(config(), metrics, stubCreds(stats), 10);

    await vi.advanceTimersByTimeAsync(10_000);
    expect(metrics.emitted[0][1].lastSyncAgeMs).toBe(0);
    bridge.close();
  });

  it("swallows emit errors (a failing tick does not throw)", async () => {
    const metrics = new FakeMetricService();
    metrics.emitError = new Error("cloudwatch down");
    const bridge = new CredentialMetricsBridge(config(), metrics, stubCreds({ secretCount: 1, syncFailures: 0, rotations: 0 }), 10);

    // No unhandled rejection / throw despite emitMetric rejecting (the await would throw otherwise).
    await vi.advanceTimersByTimeAsync(10_000);
    expect(metrics.emitted.length).toBe(0);
    bridge.close();
  });

  it("swallows errors thrown by credentials.stats()", async () => {
    const metrics = new FakeMetricService();
    const creds = stubCreds({ secretCount: 0, syncFailures: 0, rotations: 0 }, () => {
      throw new Error("vault unreadable");
    });
    const bridge = new CredentialMetricsBridge(config(), metrics, creds, 10);

    await vi.advanceTimersByTimeAsync(10_000);
    expect(metrics.emitted.length).toBe(0);
    bridge.close();
  });

  it("close() stops further emits and is idempotent", async () => {
    const metrics = new FakeMetricService();
    const bridge = new CredentialMetricsBridge(config(), metrics, stubCreds({ secretCount: 1, syncFailures: 0, rotations: 0 }), 10);

    bridge.close();
    await vi.advanceTimersByTimeAsync(30_000);
    expect(metrics.emitted.length).toBe(0);

    // Second close is a no-op (timer already cleared).
    expect(() => bridge.close()).not.toThrow();
  });
});
