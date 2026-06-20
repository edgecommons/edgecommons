import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";

import { MetricBuilder } from "../src/metrics/metric";

// Capture every PutMetricData input the target sends.
const sentInputs: Array<{ Namespace: string; MetricData: unknown[] }> = [];

// Mock the AWS SDK module so the literal `import("@aws-sdk/client-cloudwatch")` in
// cloudwatch.ts resolves to this fake (CHOICE (a): real datum/batching coverage).
vi.mock("@aws-sdk/client-cloudwatch", () => {
  class PutMetricDataCommand {
    constructor(public input: { Namespace: string; MetricData: unknown[] }) {}
  }
  class CloudWatchClient {
    async send(cmd: PutMetricDataCommand): Promise<unknown> {
      sentInputs.push(cmd.input);
      return {};
    }
  }
  return { CloudWatchClient, PutMetricDataCommand };
});

// Import after the mock is registered.
import { CloudWatchTarget } from "../src/metrics/target/cloudwatch";

function metric(largeFleet = false): ReturnType<MetricBuilder["build"]> {
  return MetricBuilder.create("requests")
    .withThingName("thing-1")
    .withComponentName("com.example.C")
    .withNamespace("ns")
    .addMeasure("count", "Count", 1)
    .build();
}

beforeEach(() => {
  sentInputs.length = 0;
});
afterEach(() => {
  vi.restoreAllMocks();
});

describe("CloudWatchTarget (mocked AWS SDK)", () => {
  it("emitNow sends datums immediately with dimensions/units", async () => {
    const t = await CloudWatchTarget.create("ns", false, 60);
    await t.emitNow(metric(), { count: 7 });
    expect(sentInputs).toHaveLength(1);
    expect(sentInputs[0].Namespace).toBe("ns");
    const datums = sentInputs[0].MetricData as Array<Record<string, unknown>>;
    expect(datums).toHaveLength(1);
    expect(datums[0].MetricName).toBe("count");
    expect(datums[0].Value).toBe(7);
    expect(datums[0].Unit).toBe("Count");
    const dims = datums[0].Dimensions as Array<{ Name: string; Value: string }>;
    expect(dims.find((d) => d.Name === "coreName")?.Value).toBe("thing-1");
    await t.shutdown();
  });

  it("emit buffers and flush sends the buffer; largeFleet adds a coreName=ALL set", async () => {
    const t = await CloudWatchTarget.create("ns", true, 60);
    await t.emit(metric(), { count: 1 });
    // Buffered: nothing sent yet.
    expect(sentInputs).toHaveLength(0);
    await t.flush();
    expect(sentInputs).toHaveLength(1);
    const datums = sentInputs[0].MetricData as Array<Record<string, unknown>>;
    // largeFleet -> normal datum + a masked-coreName datum.
    expect(datums).toHaveLength(2);
    const dims0 = datums[0].Dimensions as Array<{ Name: string; Value: string }>;
    const dims1 = datums[1].Dimensions as Array<{ Name: string; Value: string }>;
    expect(dims0.find((d) => d.Name === "coreName")?.Value).toBe("thing-1");
    expect(dims1.find((d) => d.Name === "coreName")?.Value).toBe("ALL");
    await t.shutdown();
  });

  it("flush on an empty buffer sends nothing; shutdown clears the timer", async () => {
    const t = await CloudWatchTarget.create("ns", false, 60);
    await t.flush();
    expect(sentInputs).toHaveLength(0);
    await t.shutdown();
  });

  it("send failures are logged, not thrown", async () => {
    const t = await CloudWatchTarget.create("ns", false, 60);
    const err = vi.spyOn(console, "error").mockImplementation(() => undefined);
    // Patch the client's send to throw for this emission.
    const anyT = t as unknown as { client: { send: (c: unknown) => Promise<unknown> } };
    anyT.client.send = async () => {
      throw new Error("boom");
    };
    await expect(t.emitNow(metric(), { count: 1 })).resolves.toBeUndefined();
    expect(err).toHaveBeenCalled();
    await t.shutdown();
  });
});
