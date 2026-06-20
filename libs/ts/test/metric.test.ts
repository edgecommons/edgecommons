import { describe, it, expect } from "vitest";

import { MetricBuilder, Measure } from "../src/metrics/metric";
import { buildEmf, buildEmfVariants } from "../src/metrics/emf";

describe("MetricBuilder", () => {
  it("injects category/coreName/component dimensions", () => {
    const metric = MetricBuilder.create("requests")
      .withThingName("thing-1")
      .withComponentName("com.example.C")
      .addMeasure("count", "Count", 60)
      .build();

    const dims = metric.getDimensions();
    expect(dims.get("category")).toBe("requests");
    expect(dims.get("coreName")).toBe("thing-1");
    expect(dims.get("component")).toBe("com.example.C");
  });

  it("omits coreName/component when not set, always sets category", () => {
    const metric = MetricBuilder.create("m").addMeasure("v", "None", 60).build();
    const dims = metric.getDimensions();
    expect(dims.get("category")).toBe("m");
    expect(dims.has("coreName")).toBe(false);
    expect(dims.has("component")).toBe(false);
  });

  it("Measure.storageResolution is coerced (<60 -> 1, >=60 -> 60)", () => {
    expect(new Measure("a", "Count", 1).storageResolution).toBe(1);
    expect(new Measure("a", "Count", 59).storageResolution).toBe(1);
    expect(new Measure("a", "Count", 60).storageResolution).toBe(60);
    expect(new Measure("a", "Count", 120).storageResolution).toBe(60);
  });
});

describe("EMF", () => {
  const values = { count: 7 };

  it("flattens dimensions and measures and attaches _aws metadata", () => {
    const metric = MetricBuilder.create("requests")
      .withThingName("thing-1")
      .addMeasure("count", "Count", 60)
      .build();
    const emf = buildEmf("MyApp", metric, values, false);

    expect(emf.count).toBe(7);
    expect(emf.coreName).toBe("thing-1");
    expect(emf.category).toBe("requests");

    const aws = emf._aws as Record<string, unknown>;
    const cw = (aws.CloudWatchMetrics as Array<Record<string, unknown>>)[0];
    expect(cw.Namespace).toBe("MyApp");
    const dimSet = (cw.Dimensions as string[][])[0];
    expect(dimSet).toContain("coreName");
    expect(dimSet).toContain("category");
    const metrics = cw.Metrics as Array<Record<string, unknown>>;
    expect(metrics).toEqual([{ Name: "count", Unit: "Count", StorageResolution: 60 }]);
  });

  it("_aws.Timestamp is in milliseconds (> 1e12)", () => {
    const metric = MetricBuilder.create("m").addMeasure("v", "None", 60).build();
    const emf = buildEmf("ns", metric, values, false);
    const ts = (emf._aws as Record<string, unknown>).Timestamp as number;
    expect(ts).toBeGreaterThan(1_000_000_000_000);
  });

  it("largeFleetWorkaround masks coreName to ALL", () => {
    const metric = MetricBuilder.create("m")
      .withThingName("thing-1")
      .addMeasure("v", "None", 60)
      .build();
    const emf = buildEmf("ns", metric, values, true);
    expect(emf.coreName).toBe("ALL");
  });

  it("buildEmfVariants returns 1 normally, 2 when largeFleet", () => {
    const metric = MetricBuilder.create("m")
      .withThingName("thing-1")
      .addMeasure("v", "None", 60)
      .build();
    expect(buildEmfVariants("ns", metric, values, false)).toHaveLength(1);

    const variants = buildEmfVariants("ns", metric, values, true);
    expect(variants).toHaveLength(2);
    expect(variants[0].coreName).toBe("thing-1");
    expect(variants[1].coreName).toBe("ALL");
  });
});
