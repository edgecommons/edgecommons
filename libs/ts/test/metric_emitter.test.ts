import { describe, it, expect, afterEach } from "vitest";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

import { Config } from "../src/config/model";
import { MetricEmitter } from "../src/metrics/service";
import { MetricBuilder } from "../src/metrics/metric";
import { GgError } from "../src/errors";

const tmpFiles: string[] = [];

function tmpLogPath(): string {
  const p = path.join(os.tmpdir(), `ggcommons-metric-${process.pid}-${Math.random().toString(36).slice(2)}.log`);
  tmpFiles.push(p);
  return p;
}

afterEach(() => {
  for (const f of tmpFiles.splice(0)) {
    try {
      fs.rmSync(f, { force: true });
    } catch {
      // ignore
    }
  }
});

describe("MetricEmitter (log target)", () => {
  it("writes one EMF line per emit to the log file", async () => {
    const logFile = tmpLogPath();
    const config = Config.fromValue("com.example.C", "thing-1", {
      metricEmission: { target: "log", namespace: "ns", targetConfig: { logFileName: logFile } },
    });

    const emitter = await MetricEmitter.create(config);
    const metric = MetricBuilder.create("requests")
      .withConfig(config)
      .addMeasure("count", "Count", 60)
      .build();
    emitter.defineMetric(metric);
    expect(emitter.isMetricDefined("requests")).toBe(true);

    await emitter.emitMetric("requests", { count: 1 });
    await emitter.emitMetric("requests", { count: 2 });
    await emitter.flushMetrics();
    await emitter.shutdown();

    const lines = fs.readFileSync(logFile, "utf8").trim().split("\n");
    expect(lines).toHaveLength(2);
    const first = JSON.parse(lines[0]);
    expect(first.count).toBe(1);
    expect(first.category).toBe("requests");
    expect(first.coreName).toBe("thing-1");
    expect(first._aws.CloudWatchMetrics[0].Namespace).toBe("ns");
  });

  it("emitting an undefined metric is a no-op (does not throw, writes nothing)", async () => {
    const logFile = tmpLogPath();
    const config = Config.fromValue("c", "t", {
      metricEmission: { target: "log", targetConfig: { logFileName: logFile } },
    });
    const emitter = await MetricEmitter.create(config);

    await expect(emitter.emitMetric("ghost", { x: 1 })).resolves.toBeUndefined();
    await expect(emitter.emitMetricNow("ghost", { x: 1 })).resolves.toBeUndefined();

    // Nothing defined was emitted; file may not even exist (lazy open).
    expect(fs.existsSync(logFile)).toBe(false);
  });

  it("messaging target requires a messaging service", async () => {
    const config = Config.fromValue("c", "t", {
      metricEmission: { target: "messaging" },
    });
    await expect(MetricEmitter.create(config)).rejects.toBeInstanceOf(GgError);
    try {
      await MetricEmitter.create(config);
    } catch (e) {
      expect((e as GgError).kind).toBe("Metrics");
    }
  });
});
