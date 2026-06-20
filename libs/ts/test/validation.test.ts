import { describe, it, expect } from "vitest";

import { validate } from "../src/config/validation";
import { GgError } from "../src/errors";

describe("config validation", () => {
  it("accepts a valid document", () => {
    expect(() =>
      validate({
        logging: { level: "INFO" },
        metricEmission: { target: "log", namespace: "ns" },
        heartbeat: { intervalSecs: 5, measures: { cpu: true } },
        tags: { site: "f1" },
      }),
    ).not.toThrow();
  });

  it("accepts an empty document", () => {
    expect(() => validate({})).not.toThrow();
  });

  it("rejects an invalid metricEmission.target enum value", () => {
    try {
      validate({ metricEmission: { target: "nope" } });
      throw new Error("expected validation to throw");
    } catch (e) {
      expect(e).toBeInstanceOf(GgError);
      expect((e as GgError).kind).toBe("Validation");
    }
  });
});
