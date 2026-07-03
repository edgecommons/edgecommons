/**
 * Tests for the library-owned `cfg` publisher (UNS-CANONICAL-DESIGN §4.3): the UNS topic, the
 * envelope shape (`{"config": <redacted>}` through the config-bound builder), the redaction-v1
 * rules, the config-change republish, and the best-effort failure swallowing.
 */
import { describe, expect, it } from "vitest";

import { Config } from "../src/config/model";
import { EffectiveConfigPublisher, REDACTED, redact } from "../src/config/effective_config";
import { RecordingMessagingService } from "./_fakes";

const RAW = {
  component: { global: {}, instances: [] },
  messaging: {
    local: { host: "h", port: 1883, credentials: { username: "u", password: "p" } },
  },
  tags: { site: "f1" },
};

function config(): Config {
  return Config.fromValue("com.example.C", "thing-1", RAW);
}

describe("EffectiveConfigPublisher", () => {
  it("publishes {config: <redacted>} on the UNS cfg topic through the reserved seam", async () => {
    const svc = new RecordingMessagingService();
    const publisher = new EffectiveConfigPublisher(config, svc);
    await publisher.publishNow();

    expect(svc.published).toHaveLength(1);
    const rec = svc.published[0];
    expect(rec.kind).toBe("publishReserved");
    expect(rec.topic).toBe("ecv1/thing-1/C/main/cfg");
    expect(rec.message!.header.name).toBe("cfg");
    expect(rec.message!.header.version).toBe("1.0");
    expect(rec.message!.getIdentity()?.component).toBe("C");
    const body = rec.message!.getBody() as { config: Record<string, unknown> };
    // messaging.*.credentials is redacted wholesale; non-secret values survive.
    const messaging = body.config.messaging as { local: Record<string, unknown> };
    expect(messaging.local.credentials).toBe(REDACTED);
    expect(messaging.local.host).toBe("h");
    expect(body.config.tags).toEqual({ site: "f1" });
  });

  it("republishes on a configuration change", async () => {
    const svc = new RecordingMessagingService();
    const publisher = new EffectiveConfigPublisher(config, svc);
    expect(await publisher.onConfigurationChange(config())).toBe(true);
    expect(svc.published).toHaveLength(1);
  });

  it("is best-effort: a publish failure is logged and swallowed", async () => {
    const svc = new RecordingMessagingService();
    svc.publishReserved = async () => {
      throw new Error("broker down");
    };
    const publisher = new EffectiveConfigPublisher(config, svc);
    await expect(publisher.publishNow()).resolves.toBeUndefined();
  });
});

describe("redact (redaction v1, §4.3)", () => {
  it("redacts password/pin case-insensitively anywhere, at any depth", () => {
    const out = redact({
      a: { PASSWORD: "x", nested: { pin: "1234", Pin: "5678" } },
      password: "top",
      keep: "visible",
    });
    expect(out).toEqual({
      a: { PASSWORD: REDACTED, nested: { pin: REDACTED, Pin: REDACTED } },
      password: REDACTED,
      keep: "visible",
    });
  });

  it("redacts credentials only under the TOP-LEVEL messaging section", () => {
    const out = redact({
      messaging: {
        local: { credentials: { username: "u" } },
        iotCore: { credentials: { certPath: "c" } },
      },
      streaming: { credentials: { should: "stay" } },
      nested: { messaging: { credentials: { should: "stay" } } },
    });
    const messaging = out.messaging as Record<string, Record<string, unknown>>;
    expect(messaging.local.credentials).toBe(REDACTED);
    expect(messaging.iotCore.credentials).toBe(REDACTED);
    expect((out.streaming as Record<string, unknown>).credentials).toEqual({ should: "stay" });
    expect(((out.nested as Record<string, unknown>).messaging as Record<string, unknown>).credentials).toEqual({
      should: "stay",
    });
  });

  it("walks arrays of objects and leaves $secret refs untouched (never resolved)", () => {
    const out = redact({
      streams: [{ password: "x" }, { ok: 1 }, "scalar"],
      apiKey: { $secret: "broker/apiKey" },
    });
    expect(out.streams).toEqual([{ password: REDACTED }, { ok: 1 }, "scalar"]);
    expect(out.apiKey).toEqual({ $secret: "broker/apiKey" });
  });

  it("does not mutate the input", () => {
    const input = { password: "x", messaging: { local: { credentials: { u: 1 } } } };
    redact(input);
    expect(input.password).toBe("x");
    expect(input.messaging.local.credentials).toEqual({ u: 1 });
  });
});
