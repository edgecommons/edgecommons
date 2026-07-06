import { describe, it, expect, afterEach, vi, beforeEach } from "vitest";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

import { Config } from "../src/config/model";
import { LogTarget, parseSize } from "../src/metrics/target/log";
import { MessagingMetricTarget } from "../src/metrics/target/messaging";
import { CloudWatchComponentTarget } from "../src/metrics/target/cloudwatch_component";
import { MetricBuilder } from "../src/metrics/metric";
import { Qos } from "../src/messaging/types";
import { RecordingMessagingService } from "./_fakes";

const tmpDirs: string[] = [];
function tmpDir(): string {
  const d = fs.mkdtempSync(path.join(os.tmpdir(), "ggc-mt-"));
  tmpDirs.push(d);
  return d;
}
afterEach(() => {
  for (const d of tmpDirs.splice(0)) {
    try {
      fs.rmSync(d, { recursive: true, force: true });
    } catch {
      /* ignore */
    }
  }
  vi.restoreAllMocks();
});

function metric(): ReturnType<MetricBuilder["build"]> {
  return MetricBuilder.create("requests")
    .withThingName("thing-1")
    .withComponentName("com.example.C")
    .withNamespace("ns")
    .addMeasure("count", "Count", 60)
    .build();
}

describe("LogTarget", () => {
  it("parseSize handles units and bare numbers", () => {
    expect(parseSize("10MB")).toBe(10 * 1024 * 1024);
    expect(parseSize("512KB")).toBe(512 * 1024);
    expect(parseSize("1GB")).toBe(1024 * 1024 * 1024);
    expect(parseSize("2048")).toBe(2048);
    expect(parseSize("100B")).toBe(100);
    expect(parseSize("garbage")).toBeUndefined();
    expect(parseSize("")).toBeUndefined();
  });

  it("emit writes one EMF JSON line per variant (largeFleet -> 2 lines, ALL on 2nd)", async () => {
    const dir = tmpDir();
    const file = path.join(dir, "metric.log");
    const t = new LogTarget(file, "ns", true, "10MB");
    await t.emit(metric(), { count: 5 });
    const lines = fs.readFileSync(file, "utf8").trim().split("\n");
    expect(lines).toHaveLength(2);
    const first = JSON.parse(lines[0]);
    const second = JSON.parse(lines[1]);
    expect(first.count).toBe(5);
    expect(first.coreName).toBe("thing-1");
    expect(second.coreName).toBe("ALL");
    expect(first._aws.CloudWatchMetrics[0].Namespace).toBe("ns");
  });

  it("rotates when maxFileSize is exceeded and keeps <=5 backups", async () => {
    const dir = tmpDir();
    const file = path.join(dir, "m.log");
    // Tiny limit so each emit (one line) rotates the previous file.
    const t = new LogTarget(file, "ns", false, "200B");
    for (let i = 0; i < 20; i++) {
      await t.emit(metric(), { count: i });
    }
    const entries = fs.readdirSync(dir);
    const backups = entries.filter((e) => e.startsWith("m-") && e.endsWith(".log"));
    expect(backups.length).toBeGreaterThan(0);
    expect(backups.length).toBeLessThanOrEqual(5);
    // The active file still exists.
    expect(fs.existsSync(file)).toBe(true);
  });

  it("fail-soft when the path is unwritable (a directory): no throw, warns once", async () => {
    const dir = tmpDir(); // use the directory itself as the 'file' path -> unwritable
    const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    const t = new LogTarget(dir, "ns", false, "10MB");
    await expect(t.emit(metric(), { count: 1 })).resolves.toBeUndefined();
    await expect(t.emitNow(metric(), { count: 2 })).resolves.toBeUndefined();
    expect(warn).toHaveBeenCalledTimes(1);
    await t.flush();
    await t.shutdown();
  });
});

describe("MessagingMetricTarget (UNS metric topics, §4.3)", () => {
  const config = Config.fromValue("com.example.C", "thing-1", { tags: { site: "f1" } });
  const METRIC_TOPIC = "ecv1/thing-1/C/main/metric/requests";

  it("wraps EMF in a Metric/1.0 envelope and publishes the UNS topic through the reserved seam", async () => {
    const svc = new RecordingMessagingService();
    const t = new MessagingMetricTarget(svc, config, false, "ns", false);
    await t.emit(metric(), { count: 3 });
    expect(svc.published).toHaveLength(1);
    const rec = svc.published[0];
    expect(rec.kind).toBe("publishReserved");
    expect(rec.topic).toBe(METRIC_TOPIC);
    expect(rec.message!.header.name).toBe("Metric");
    expect(rec.message!.header.version).toBe("1.0");
    expect(rec.message!.tags?.site).toBe("f1");
    // The envelope carries the resolved component identity (no legacy tags.thing).
    expect(rec.message!.getIdentity()?.device).toBe("thing-1");
    expect(rec.message!.getIdentity()?.component).toBe("C");
    expect("thing" in (rec.message!.tags ?? {})).toBe(false);
    const body = rec.message!.getBody() as Record<string, unknown>;
    expect(body.count).toBe(3);
  });

  it("routes to the northbound broker with AtLeastOnce when selected", async () => {
    const svc = new RecordingMessagingService();
    const t = new MessagingMetricTarget(svc, config, true, "ns", false);
    await t.emitNow(metric(), { count: 1 });
    expect(svc.published[0].kind).toBe("publishReservedNorthbound");
    expect(svc.published[0].topic).toBe(METRIC_TOPIC);
    expect(svc.published[0].qos).toBe(Qos.AtLeastOnce);
  });

  it("sanitizes the metric name into the channel token", async () => {
    const svc = new RecordingMessagingService();
    const t = new MessagingMetricTarget(svc, config, false, "ns", false);
    const weird = MetricBuilder.create("req/count+all")
      .withThingName("thing-1")
      .addMeasure("count", "Count", 60)
      .build();
    await t.emit(weird, { count: 1 });
    expect(svc.published[0].topic).toBe("ecv1/thing-1/C/main/metric/req_count_all");
  });

  it("largeFleetWorkaround emits 2 variants (coreName ALL on the 2nd)", async () => {
    const svc = new RecordingMessagingService();
    const t = new MessagingMetricTarget(svc, config, false, "ns", true);
    await t.emit(metric(), { count: 1 });
    expect(svc.published).toHaveLength(2);
    const b0 = svc.published[0].message!.getBody() as Record<string, unknown>;
    const b1 = svc.published[1].message!.getBody() as Record<string, unknown>;
    expect(b0.coreName).toBe("thing-1");
    expect(b1.coreName).toBe("ALL");
    await t.flush();
    await t.shutdown();
  });
});

describe("CloudWatchComponentTarget", () => {
  it("publishes one raw PutMetricData request per measure, excluding coreName dims", async () => {
    const svc = new RecordingMessagingService();
    const t = new CloudWatchComponentTarget(svc, "cloudwatch/metric/put", "ns");
    await t.emit(metric(), { count: 4, latency: 12 });
    expect(svc.published).toHaveLength(2);
    for (const rec of svc.published) {
      expect(rec.kind).toBe("publishRaw");
      expect(rec.topic).toBe("cloudwatch/metric/put");
    }
    const p0 = svc.published[0].payload as { request: { namespace: string; metricData: Record<string, unknown> } };
    expect(p0.request.namespace).toBe("ns");
    const md = p0.request.metricData;
    expect(md.metricName).toBe("count");
    expect(md.value).toBe(4);
    expect(md.unit).toBe("Count");
    expect(typeof md.timestamp).toBe("number");
    // dimensions array excludes coreName.
    const dims = md.dimensions as Array<{ name: string; value: string }>;
    expect(dims.find((d) => d.name === "coreName")).toBeUndefined();
    expect(dims.find((d) => d.name === "category")?.value).toBe("requests");
    await t.flush();
    await t.shutdown();
  });
});
