import { describe, it, expect } from "vitest";

import { Config } from "../src/config/model";

describe("Config.fromValue", () => {
  it("parses identity and basic sections", () => {
    const cfg = Config.fromValue("com.example.MyComponent", "thing-7", {
      tags: { site: "factory-1" },
    });
    expect(cfg.componentName).toBe("com.example.MyComponent");
    expect(cfg.thingName).toBe("thing-7");
    expect(cfg.parsed.tags).toEqual({ site: "factory-1" });
  });

  it("instanceIds() and instance() read component.instances", () => {
    const cfg = Config.fromValue("c", "t", {
      component: {
        global: { g: 1 },
        instances: [
          { id: "a", v: 1 },
          { id: "b", v: 2 },
        ],
      },
    });
    expect(cfg.instanceIds()).toEqual(["a", "b"]);
    expect(cfg.instance("b")).toEqual({ id: "b", v: 2 });
    expect(cfg.instance("missing")).toBeUndefined();
    expect(cfg.global()).toEqual({ g: 1 });
  });

  describe("MetricConfig defaulting accessors", () => {
    it("applies defaults when metricEmission is empty", () => {
      const mc = Config.fromValue("c", "t", {}).parsed.metricEmission;
      expect(mc.target()).toBe("log");
      expect(mc.namespace()).toBe("ggcommons");
      expect(mc.destination()).toBe("ipc");
      expect(mc.intervalSecs()).toBe(5);
      expect(mc.logFileName()).toContain("{ComponentFullName}");
      expect(mc.topic()).toBe("{ThingName}/{ComponentName}/metric");
    });

    it("cloudwatchcomponent target topic default", () => {
      const mc = Config.fromValue("c", "t", {
        metricEmission: { target: "cloudwatchcomponent" },
      }).parsed.metricEmission;
      expect(mc.topic()).toBe("cloudwatch/metric/put");
    });

    it("intervalSecs has a minimum of 1", () => {
      const mc = Config.fromValue("c", "t", {
        metricEmission: { targetConfig: { intervalSecs: 0 } },
      }).parsed.metricEmission;
      expect(mc.intervalSecs()).toBe(5);
    });

    it("intervalSecs accepts a float (10.0 -> 10)", () => {
      const mc = Config.fromValue("c", "t", {
        metricEmission: { targetConfig: { intervalSecs: 10.0 } },
      }).parsed.metricEmission;
      expect(mc.intervalSecs()).toBe(10);
    });

    it("honors explicit overrides", () => {
      const mc = Config.fromValue("c", "t", {
        metricEmission: {
          target: "messaging",
          namespace: "MyNs",
          targetConfig: { destination: "iotcore", topic: "x/y" },
        },
      }).parsed.metricEmission;
      expect(mc.target()).toBe("messaging");
      expect(mc.namespace()).toBe("MyNs");
      expect(mc.destination()).toBe("iotcore");
      expect(mc.topic()).toBe("x/y");
    });
  });

  describe("HeartbeatConfig numeric leniency", () => {
    it("intervalSecs accepts a float", () => {
      const hb = Config.fromValue("c", "t", {
        heartbeat: { intervalSecs: 10.0 },
      }).parsed.heartbeat;
      expect(hb.intervalSecs).toBe(10);
    });

    it("parses measures and targets", () => {
      const hb = Config.fromValue("c", "t", {
        heartbeat: {
          measures: { cpu: true, memory: true },
          targets: [{ type: "metric" }, { type: "messaging", config: { topic: "t" } }],
        },
      }).parsed.heartbeat;
      expect(hb.measures.cpu).toBe(true);
      expect(hb.measures.memory).toBe(true);
      expect(hb.measures.disk).toBe(false);
      expect(hb.targets).toHaveLength(2);
      expect(hb.targets[1].type).toBe("messaging");
      expect(hb.targets[1].config).toEqual({ topic: "t" });
    });
  });

  describe("HealthConfig parsing (Phase 1c / FR-HB-1)", () => {
    it("applies schema defaults when health is absent (enabled tri-state undefined)", () => {
      const h = Config.fromValue("c", "t", {}).parsed.health;
      expect(h.enabled).toBeUndefined(); // no key -> defer to the platform default
      expect(h.port).toBe(8081);
      expect(h.livenessPath).toBe("/livez");
      expect(h.readinessPath).toBe("/readyz");
      expect(h.startupPath).toBe("/startupz");
    });

    it("preserves an explicit enabled=false (distinct from absent)", () => {
      expect(Config.fromValue("c", "t", { health: { enabled: false } }).parsed.health.enabled).toBe(false);
      expect(Config.fromValue("c", "t", { health: { enabled: true } }).parsed.health.enabled).toBe(true);
    });

    it("honors explicit port (incl. float) and custom paths", () => {
      const h = Config.fromValue("c", "t", {
        health: { enabled: true, port: 9090.0, livenessPath: "/l", readinessPath: "/r", startupPath: "/s" },
      }).parsed.health;
      expect(h.port).toBe(9090);
      expect(h.livenessPath).toBe("/l");
      expect(h.readinessPath).toBe("/r");
      expect(h.startupPath).toBe("/s");
    });
  });

  describe("FileLoggingConfig defaults", () => {
    it("defaults maxFileSize and backupCount", () => {
      const lc = Config.fromValue("c", "t", {
        logging: { fileLogging: { enabled: true } },
      }).parsed.logging;
      expect(lc.fileLogging?.enabled).toBe(true);
      expect(lc.fileLogging?.maxFileSize()).toBe("10MB");
      expect(lc.fileLogging?.backupCount()).toBe(5);
    });

    it("honors explicit fileLogging values", () => {
      const lc = Config.fromValue("c", "t", {
        logging: { fileLogging: { enabled: true, maxFileSize: "20MB", backupCount: 3 } },
      }).parsed.logging;
      expect(lc.fileLogging?.maxFileSize()).toBe("20MB");
      expect(lc.fileLogging?.backupCount()).toBe(3);
    });
  });
});
