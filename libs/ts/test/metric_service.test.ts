import { describe, it, expect, vi, afterEach } from "vitest";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

import { Config } from "../src/config/model";
import { MetricEmitter } from "../src/metrics/service";
import { LogTarget } from "../src/metrics/target/log";
import { MessagingMetricTarget } from "../src/metrics/target/messaging";
import { CloudWatchComponentTarget } from "../src/metrics/target/cloudwatch_component";
import { MetricBuilder } from "../src/metrics/metric";
import { RecordingMessagingService } from "./_fakes";

// Mock the AWS SDK so the 'cloudwatch' target selection succeeds without AWS.
vi.mock("@aws-sdk/client-cloudwatch", () => {
  class PutMetricDataCommand {
    constructor(public input: unknown) {}
  }
  class CloudWatchClient {
    async send(): Promise<unknown> {
      return {};
    }
  }
  return { CloudWatchClient, PutMetricDataCommand };
});
import { CloudWatchTarget } from "../src/metrics/target/cloudwatch";

const tmp: string[] = [];
function tmpLog(): string {
  const p = path.join(os.tmpdir(), `ggc-svc-${Math.random().toString(36).slice(2)}.log`);
  tmp.push(p);
  return p;
}
afterEach(() => {
  for (const f of tmp.splice(0)) {
    try {
      fs.rmSync(f, { force: true });
    } catch {
      /* ignore */
    }
  }
  vi.restoreAllMocks();
});

/** Reach the private `target` field for type assertions. */
function targetOf(e: MetricEmitter): unknown {
  return (e as unknown as { target: unknown }).target;
}

function cfg(metricEmission: Record<string, unknown>): Config {
  return Config.fromValue("com.example.C", "thing-1", { metricEmission });
}

describe("MetricEmitter.buildTarget selection", () => {
  it("selects LogTarget for 'log'", async () => {
    const e = await MetricEmitter.create(cfg({ target: "log", targetConfig: { logFileName: tmpLog() } }));
    expect(targetOf(e)).toBeInstanceOf(LogTarget);
  });

  it("selects MessagingMetricTarget for 'messaging'", async () => {
    const svc = new RecordingMessagingService();
    const e = await MetricEmitter.create(cfg({ target: "messaging" }), svc);
    expect(targetOf(e)).toBeInstanceOf(MessagingMetricTarget);
  });

  it("selects CloudWatchComponentTarget for 'cloudwatchcomponent'", async () => {
    const svc = new RecordingMessagingService();
    const e = await MetricEmitter.create(cfg({ target: "cloudwatchcomponent" }), svc);
    expect(targetOf(e)).toBeInstanceOf(CloudWatchComponentTarget);
  });

  it("selects CloudWatchTarget for 'cloudwatch' (mocked SDK present)", async () => {
    const e = await MetricEmitter.create(cfg({ target: "cloudwatch" }));
    expect(targetOf(e)).toBeInstanceOf(CloudWatchTarget);
    await e.shutdown();
  });

  it("unknown target warns and defaults to LogTarget", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    const e = await MetricEmitter.create(cfg({ target: "bogus", targetConfig: { logFileName: tmpLog() } }));
    expect(targetOf(e)).toBeInstanceOf(LogTarget);
    expect(warn).toHaveBeenCalled();
  });
});

describe("MetricEmitter lifecycle", () => {
  it("defineMetric/isMetricDefined and undefined-metric no-op", async () => {
    const e = await MetricEmitter.create(cfg({ target: "log", targetConfig: { logFileName: tmpLog() } }));
    const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    expect(e.isMetricDefined("requests")).toBe(false);
    e.defineMetric(MetricBuilder.create("requests").addMeasure("count", "Count", 60).build());
    expect(e.isMetricDefined("requests")).toBe(true);
    await expect(e.emitMetric("ghost", { x: 1 })).resolves.toBeUndefined();
    await expect(e.emitMetricNow("ghost", { x: 1 })).resolves.toBeUndefined();
    expect(warn).toHaveBeenCalled();
    await e.flushMetrics();
    await e.shutdown();
  });

  it("onConfigurationChange rebuilds the target and routes emits to the new one", async () => {
    const svc = new RecordingMessagingService();
    // Start with a log target...
    const e = await MetricEmitter.create(cfg({ target: "log", targetConfig: { logFileName: tmpLog() } }), svc);
    expect(targetOf(e)).toBeInstanceOf(LogTarget);
    e.defineMetric(MetricBuilder.create("requests").withConfig(cfg({})).addMeasure("count", "Count", 60).build());

    // ...rebuild to a messaging target on config change.
    const changed = cfg({ target: "messaging", targetConfig: { topic: "m/t" } });
    expect(await e.onConfigurationChange(changed)).toBe(true);
    expect(targetOf(e)).toBeInstanceOf(MessagingMetricTarget);

    await e.emitMetricNow("requests", { count: 9 });
    expect(svc.published).toHaveLength(1);
    expect(svc.published[0].topic).toBe("m/t");
  });

  it("onConfigurationChange keeps the previous target on rebuild error", async () => {
    // Start with a valid messaging target.
    const svc = new RecordingMessagingService();
    const e = await MetricEmitter.create(cfg({ target: "messaging", targetConfig: { topic: "m/t" } }), svc);
    const prev = targetOf(e);
    const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    // No messaging service is retained? It is. Force an error: messaging target on an
    // emitter created WITHOUT messaging would throw. Recreate that scenario:
    const eNoMsg = await MetricEmitter.create(cfg({ target: "log", targetConfig: { logFileName: tmpLog() } }));
    const prevNoMsg = targetOf(eNoMsg);
    expect(await eNoMsg.onConfigurationChange(cfg({ target: "messaging" }))).toBe(false);
    expect(targetOf(eNoMsg)).toBe(prevNoMsg);
    expect(warn).toHaveBeenCalled();
    expect(targetOf(e)).toBe(prev);
  });
});

describe("MetricEmitter cloudwatch absent", () => {
  it("create('cloudwatch') throws GgError(Metrics) when the SDK import fails", async () => {
    // Re-import cloudwatch with a mock that throws, isolating the module registry.
    await vi.resetModules();
    vi.doMock("@aws-sdk/client-cloudwatch", () => {
      throw new Error("Cannot find module");
    });
    const { MetricEmitter: FreshEmitter } = await import("../src/metrics/service");
    const { Config: FreshConfig } = await import("../src/config/model");
    const { GgError } = await import("../src/errors");
    const c = FreshConfig.fromValue("c", "t", { metricEmission: { target: "cloudwatch" } });
    await expect(FreshEmitter.create(c)).rejects.toBeInstanceOf(GgError);
    await FreshEmitter.create(c).catch((e) => expect((e as InstanceType<typeof GgError>).kind).toBe("Metrics"));
    vi.doUnmock("@aws-sdk/client-cloudwatch");
    await vi.resetModules();
  });
});
