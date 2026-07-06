import { describe, it, expect } from "vitest";

import { validate } from "../src/config/validation";
import { EdgeCommonsError } from "../src/errors";

describe("config validation", () => {
  it("accepts a valid document", () => {
    expect(() =>
      validate({
        component: { global: {} },
        logging: { level: "INFO" },
        metricEmission: { target: "log", namespace: "ns" },
        heartbeat: { intervalSecs: 5, measures: { cpu: true } },
        tags: { site: "f1" },
      }),
    ).not.toThrow();
  });

  it("rejects a document with no component section", () => {
    // The canonical cross-language schema requires a top-level `component` section.
    try {
      validate({});
      throw new Error("expected validation to throw");
    } catch (e) {
      expect(e).toBeInstanceOf(EdgeCommonsError);
      expect((e as EdgeCommonsError).kind).toBe("Validation");
    }
  });

  it("rejects an invalid metricEmission.target enum value", () => {
    try {
      validate({ metricEmission: { target: "nope" } });
      throw new Error("expected validation to throw");
    } catch (e) {
      expect(e).toBeInstanceOf(EdgeCommonsError);
      expect((e as EdgeCommonsError).kind).toBe("Validation");
    }
  });

  it("accepts the UNS sections (hierarchy/identity/topic + messaging additions)", () => {
    expect(() =>
      validate({
        component: { global: {} },
        hierarchy: { levels: ["site", "zone", "device"] },
        identity: { site: "dallas", zone: "zone-3" },
        topic: { includeRoot: true },
        heartbeat: { enabled: true, intervalSecs: 5, measures: { cpu: true }, destination: "local" },
        messaging: {
          requestTimeoutSeconds: 30,
          lwt: { topic: "ecv1/gw/c/main/state", payload: { status: "UNREACHABLE" }, qos: 1 },
        },
      }),
    ).not.toThrow();
  });

  it("rejects the removed drift knobs (heartbeat.targets / metricEmission.targetConfig.topic)", () => {
    // The UNS hard cut removed these from the schema; stale configs must fail with a precise error.
    expect(() => validate({ component: {}, heartbeat: { targets: [{ type: "metric" }] } })).toThrow(EdgeCommonsError);
    expect(() => validate({ component: {}, metricEmission: { targetConfig: { topic: "x/y" } } })).toThrow(EdgeCommonsError);
  });

  it("rejects an lwt without a topic and an out-of-range lwt qos", () => {
    expect(() => validate({ component: {}, messaging: { lwt: { payload: "x" } } })).toThrow(EdgeCommonsError);
    expect(() => validate({ component: {}, messaging: { lwt: { topic: "t", qos: 2 } } })).toThrow(EdgeCommonsError);
  });
});
