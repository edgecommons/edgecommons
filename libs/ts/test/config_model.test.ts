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
      expect(mc.explicitTarget()).toBeUndefined();
      expect(mc.namespace()).toBe("edgecommons");
      expect(mc.destination()).toBe("ipc");
      expect(mc.intervalSecs()).toBe(5);
      expect(mc.logFileName()).toContain("{ComponentFullName}");
      expect(mc.explicitLogFileName()).toBeUndefined();
      // prometheus target accessors (FR-MET-1): schema defaults port 9090, path /metrics.
      expect(mc.prometheusPort()).toBe(9090);
      expect(mc.prometheusPath()).toBe("/metrics");
    });

    it("explicitLogFileName reflects the configured value (HOST-aware path precedence)", () => {
      const mc = Config.fromValue("c", "t", {
        metricEmission: { target: "log", targetConfig: { logFileName: "/custom/x.log" } },
      }).parsed.metricEmission;
      expect(mc.explicitLogFileName()).toBe("/custom/x.log");
      expect(mc.logFileName()).toBe("/custom/x.log");
    });

    it("prometheus target port/path overrides + invalid-port fallback", () => {
      const mc = Config.fromValue("c", "t", {
        metricEmission: { target: "prometheus", targetConfig: { port: 9123, path: "/prom" } },
      }).parsed.metricEmission;
      expect(mc.target()).toBe("prometheus");
      expect(mc.explicitTarget()).toBe("prometheus");
      expect(mc.prometheusPort()).toBe(9123);
      expect(mc.prometheusPath()).toBe("/prom");
      // Out-of-range / non-numeric port falls back to the 9090 default.
      const bad = Config.fromValue("c", "t", {
        metricEmission: { targetConfig: { port: 0 } },
      }).parsed.metricEmission;
      expect(bad.prometheusPort()).toBe(9090);
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
          targetConfig: { destination: "iotcore" },
        },
      }).parsed.metricEmission;
      expect(mc.target()).toBe("messaging");
      expect(mc.namespace()).toBe("MyNs");
      expect(mc.destination()).toBe("iotcore");
    });
  });

  describe("HeartbeatConfig (UNS reshape, D-U14/D-U20)", () => {
    it("defaults: on / 5 s / local, cpu+memory measures on", () => {
      const hb = Config.fromValue("c", "t", {}).parsed.heartbeat;
      expect(hb.enabled).toBe(true);
      expect(hb.intervalSecs).toBe(5);
      expect(hb.destination).toBe("local");
      expect(hb.measures.cpu).toBe(true);
      expect(hb.measures.memory).toBe(true);
      expect(hb.measures.disk).toBe(false);
      expect(hb.measures.threads).toBe(false);
    });

    it("intervalSecs accepts a float and clamps out-of-range values to the 5s default", () => {
      expect(Config.fromValue("c", "t", { heartbeat: { intervalSecs: 10.0 } }).parsed.heartbeat.intervalSecs).toBe(10);
      expect(Config.fromValue("c", "t", { heartbeat: { intervalSecs: 0 } }).parsed.heartbeat.intervalSecs).toBe(5);
    });

    it("parses enabled, measures overrides, and destination", () => {
      const hb = Config.fromValue("c", "t", {
        heartbeat: {
          enabled: false,
          measures: { cpu: false, memory: true, disk: true },
          destination: "iotcore",
        },
      }).parsed.heartbeat;
      expect(hb.enabled).toBe(false);
      expect(hb.measures.cpu).toBe(false);
      expect(hb.measures.memory).toBe(true);
      expect(hb.measures.disk).toBe(true);
      expect(hb.destination).toBe("iotcore");
    });
  });

  describe("UNS identity resolution (§1.5) + topic/messaging knobs", () => {
    it("zero-config default: single 'device' level from the sanitized thing name + short component token", () => {
      const cfg = Config.fromValue("com.example.MyComponent", "thing-7", {});
      const id = cfg.componentIdentity;
      expect(id.hier).toEqual([{ level: "device", value: "thing-7" }]);
      expect(id.path).toBe("thing-7");
      expect(id.device).toBe("thing-7");
      expect(id.component).toBe("MyComponent");
      expect(id.instance).toBe("main");
      expect(cfg.topicIncludeRoot).toBe(false);
      expect(cfg.messagingRequestTimeoutSeconds).toBe(30);
      expect(cfg.messagingRequestTimeoutMs()).toBe(30_000);
    });

    it("uses component.token when configured so PascalCase component names keep lower-kebab UNS tokens", () => {
      const cfg = Config.fromValue("com.mbreissi.edgecommons.OpcUaAdapter", "thing-7", {
        component: { token: "opcua-adapter" },
      });
      expect(cfg.componentIdentity.component).toBe("opcua-adapter");
    });

    it("resolves a multi-level hierarchy with values from the identity object", () => {
      const cfg = Config.fromValue("com.example.C", "gw-01", {
        hierarchy: { levels: ["site", "zone", "device"] },
        identity: { site: "dallas", zone: "zone-3" },
        topic: { includeRoot: true },
      });
      expect(cfg.componentIdentity.hier).toEqual([
        { level: "site", value: "dallas" },
        { level: "zone", value: "zone-3" },
        { level: "device", value: "gw-01" },
      ]);
      expect(cfg.componentIdentity.path).toBe("dallas/zone-3/gw-01");
      expect(cfg.topicIncludeRoot).toBe(true);
    });

    it("sanitizes identity values, the thing name and the component token", () => {
      const cfg = Config.fromValue("com.example.My+Comp", "gw/01", {
        hierarchy: { levels: ["site", "device"] },
        identity: { site: "dal+las" },
      });
      expect(cfg.componentIdentity.hier).toEqual([
        { level: "site", value: "dal_las" },
        { level: "device", value: "gw_01" },
      ]);
      expect(cfg.componentIdentity.component).toBe("My_Comp");
    });

    it("fails fast on identity/hierarchy inconsistencies", () => {
      // Missing value for a non-device level.
      expect(() =>
        Config.fromValue("c", "t", { hierarchy: { levels: ["site", "device"] } }),
      ).toThrow(/missing value\(s\) for hierarchy level\(s\) \[site\]/);
      // A key equal to the device level.
      expect(() =>
        Config.fromValue("c", "t", {
          hierarchy: { levels: ["site", "device"] },
          identity: { site: "s", device: "nope" },
        }),
      ).toThrow(/'identity.device' must not be set/);
      // A key that is not a declared level.
      expect(() =>
        Config.fromValue("c", "t", {
          hierarchy: { levels: ["site", "device"] },
          identity: { site: "s", bogus: "x" },
        }),
      ).toThrow(/'identity.bogus' is not a declared hierarchy level/);
      // Malformed hierarchy shapes.
      expect(() => Config.fromValue("c", "t", { hierarchy: {} })).toThrow(/'hierarchy' must be an object/);
      expect(() => Config.fromValue("c", "t", { hierarchy: { levels: [] } })).toThrow(/non-empty array/);
      expect(() => Config.fromValue("c", "t", { hierarchy: { levels: [42] } })).toThrow(/must be strings/);
      expect(() => Config.fromValue("c", "t", { hierarchy: { levels: ["bad name"] } })).toThrow(
        /invalid hierarchy level name/,
      );
      expect(() => Config.fromValue("c", "t", { hierarchy: { levels: ["a", "a"] } })).toThrow(/duplicate/);
      expect(() => Config.fromValue("c", "t", { identity: "nope" })).toThrow(/'identity' must be an object/);
      // Missing thing name / component name.
      expect(() => Config.fromValue("c", "", {})).toThrow(/resolved thing name/);
      expect(() => Config.fromValue("", "t", {})).toThrow(/component name/);
      expect(() => Config.fromValue("c", "t", { component: { token: "" } })).toThrow(/component\.token/);
    });

    it("parses messaging.requestTimeoutSeconds (0 = disabled; negative/malformed -> default)", () => {
      expect(
        Config.fromValue("c", "t", { messaging: { requestTimeoutSeconds: 0 } }).messagingRequestTimeoutMs(),
      ).toBe(0);
      expect(
        Config.fromValue("c", "t", { messaging: { requestTimeoutSeconds: 2.5 } }).messagingRequestTimeoutMs(),
      ).toBe(2500);
      expect(
        Config.fromValue("c", "t", { messaging: { requestTimeoutSeconds: -1 } }).messagingRequestTimeoutMs(),
      ).toBe(30_000);
      expect(
        Config.fromValue("c", "t", { messaging: { requestTimeoutSeconds: "x" } }).messagingRequestTimeoutMs(),
      ).toBe(30_000);
    });

    it("topic.includeRoot is lenient (non-boolean/absent -> false)", () => {
      expect(Config.fromValue("c", "t", { topic: {} }).topicIncludeRoot).toBe(false);
      expect(Config.fromValue("c", "t", { topic: { includeRoot: "yes" } }).topicIncludeRoot).toBe(false);
      // includeRoot on a single-level hierarchy parses true but WARNs (a no-op in Uns, D-U25).
      expect(Config.fromValue("c", "t", { topic: { includeRoot: true } }).topicIncludeRoot).toBe(true);
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
