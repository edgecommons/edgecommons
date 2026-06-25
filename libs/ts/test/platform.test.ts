/**
 * Unit tests for the pure platform resolver (`src/platform.ts`) — the precedence resolver
 * (DESIGN-core §4), the auto-detector (§5), the IPC-lock validation (§4.1), and identity resolution
 * (§6.2). Mirrors the canonical Java `PlatformResolverTest`. Exercised in isolation with injected
 * environments and a filesystem probe so the suite is the Phase-0 oracle.
 */
import { describe, it, expect } from "vitest";

import {
  DEFAULT_IDENTITY,
  ENV_GG_IPC_SOCKET,
  ENV_GG_SVCUID,
  ENV_K8S_SERVICE_HOST,
  ENV_THING_NAME,
  K8S_SA_TOKEN_PATH,
  PROFILES,
  Platform,
  Transport,
  detectPlatform,
  resolveIdentity,
  resolveProfile,
  validate,
} from "../src/platform";
import { GgError } from "../src/errors";

const NO_FILES = (): boolean => false;
const ALL_FILES = (): boolean => true;

describe("detectPlatform", () => {
  it("GREENGRASS from the IPC-socket env", () => {
    expect(detectPlatform({ [ENV_GG_IPC_SOCKET]: "/run/gg.sock" }, NO_FILES)).toBe(Platform.GREENGRASS);
  });

  it("GREENGRASS from the SVCUID env", () => {
    expect(detectPlatform({ [ENV_GG_SVCUID]: "abc123" }, NO_FILES)).toBe(Platform.GREENGRASS);
  });

  it("KUBERNETES from the projected SA token file", () => {
    const onlyToken = (p: string): boolean => p === K8S_SA_TOKEN_PATH;
    expect(detectPlatform({}, onlyToken)).toBe(Platform.KUBERNETES);
  });

  it("KUBERNETES from the service-host env", () => {
    expect(detectPlatform({ [ENV_K8S_SERVICE_HOST]: "10.0.0.1" }, NO_FILES)).toBe(Platform.KUBERNETES);
  });

  it("HOST when no signals", () => {
    expect(detectPlatform({}, NO_FILES)).toBe(Platform.HOST);
  });

  it("GREENGRASS wins over KUBERNETES when both signals present (load-bearing order)", () => {
    const env = { [ENV_GG_SVCUID]: "uid", [ENV_K8S_SERVICE_HOST]: "10.0.0.1" };
    expect(detectPlatform(env, ALL_FILES)).toBe(Platform.GREENGRASS);
  });

  it("an empty env value is not a signal", () => {
    expect(detectPlatform({ [ENV_GG_SVCUID]: "" }, NO_FILES)).toBe(Platform.HOST);
  });

  it("public detect uses the real filesystem probe (token absent on host -> HOST)", () => {
    expect(detectPlatform({})).toBe(Platform.HOST);
  });
});

describe("resolveProfile: profile defaults", () => {
  it("explicit GREENGRASS -> IPC + GG_CONFIG", () => {
    const r = resolveProfile({ platform: Platform.GREENGRASS }, {});
    expect(r.platform).toBe(Platform.GREENGRASS);
    expect(r.transport).toBe(Transport.IPC);
    expect(r.configSource).toEqual(["GG_CONFIG"]);
    expect(r.identity).toBe(DEFAULT_IDENTITY);
  });

  it("explicit HOST -> MQTT + GG_CONFIG in Phase 0 (not FILE)", () => {
    const r = resolveProfile({ platform: Platform.HOST }, {});
    expect(r.platform).toBe(Platform.HOST);
    expect(r.transport).toBe(Transport.MQTT);
    expect(r.configSource).toEqual(["GG_CONFIG"]);
  });

  it("auto with no signals detects HOST", () => {
    const r = resolveProfile({}, {});
    expect(r.platform).toBe(Platform.HOST);
    expect(r.transport).toBe(Transport.MQTT);
  });

  it("auto with a Greengrass env detects GREENGRASS", () => {
    const r = resolveProfile({}, { [ENV_GG_IPC_SOCKET]: "/run/gg.sock" });
    expect(r.platform).toBe(Platform.GREENGRASS);
    expect(r.transport).toBe(Transport.IPC);
  });
});

describe("resolveProfile: explicit overrides", () => {
  it("explicit config args override the profile default", () => {
    const r = resolveProfile(
      { platform: Platform.GREENGRASS, configArgs: ["FILE", "/etc/cfg.json"] },
      {},
    );
    expect(r.configSource).toEqual(["FILE", "/etc/cfg.json"]);
  });

  it("explicit transport overrides the profile default", () => {
    const r = resolveProfile({ platform: Platform.HOST, transport: Transport.MQTT }, {});
    expect(r.transport).toBe(Transport.MQTT);
  });

  it("explicit thing overrides the env probe", () => {
    const r = resolveProfile({ platform: Platform.HOST, thing: "my-thing" }, {
      [ENV_THING_NAME]: "env-thing",
    });
    expect(r.identity).toBe("my-thing");
  });
});

describe("resolveProfile: failures", () => {
  it("KUBERNETES fails fast in Phase 0", () => {
    expect(() => resolveProfile({ platform: Platform.KUBERNETES }, {})).toThrow(/KUBERNETES/);
  });

  it("IPC on HOST fails the IPC lock", () => {
    expect(() => resolveProfile({ platform: Platform.HOST, transport: Transport.IPC }, {})).toThrow(
      /IPC transport requires --platform GREENGRASS/,
    );
  });

  it("resolver failures are GgError of kind Cli", () => {
    try {
      resolveProfile({ platform: Platform.KUBERNETES }, {});
      throw new Error("expected throw");
    } catch (e) {
      expect(e).toBeInstanceOf(GgError);
      expect((e as GgError).kind).toBe("Cli");
    }
  });
});

describe("validate", () => {
  it("rejects IPC on non-Greengrass", () => {
    expect(() => validate(Platform.HOST, Transport.IPC)).toThrow(GgError);
    expect(() => validate(Platform.KUBERNETES, Transport.IPC)).toThrow(GgError);
  });

  it("accepts legal combos", () => {
    expect(() => validate(Platform.GREENGRASS, Transport.IPC)).not.toThrow();
    expect(() => validate(Platform.HOST, Transport.MQTT)).not.toThrow();
    expect(() => validate(Platform.KUBERNETES, Transport.MQTT)).not.toThrow();
  });
});

describe("resolveIdentity", () => {
  it("prefers the explicit thing", () => {
    expect(resolveIdentity("t1", Platform.GREENGRASS, {})).toBe("t1");
  });

  it("falls back to the env probe", () => {
    expect(resolveIdentity(undefined, Platform.HOST, { [ENV_THING_NAME]: "env-thing" })).toBe(
      "env-thing",
    );
  });

  it("defaults when nothing is available", () => {
    expect(resolveIdentity(undefined, Platform.HOST, {})).toBe(DEFAULT_IDENTITY);
  });

  it("handles an undefined env", () => {
    expect(resolveIdentity(undefined, Platform.HOST, undefined)).toBe(DEFAULT_IDENTITY);
  });
});

describe("profiles + enums", () => {
  it("PROFILES contains only GREENGRASS and HOST in Phase 0", () => {
    expect(PROFILES.size).toBe(2);
    expect(PROFILES.has(Platform.GREENGRASS)).toBe(true);
    expect(PROFILES.has(Platform.HOST)).toBe(true);
    expect(PROFILES.has(Platform.KUBERNETES)).toBe(false);
  });

  it("enums declare the expected values", () => {
    expect(Object.keys(Platform)).toEqual(["GREENGRASS", "HOST", "KUBERNETES"]);
    expect(Object.keys(Transport)).toEqual(["IPC", "MQTT"]);
  });

  it("the GREENGRASS profile exposes its fields", () => {
    const p = PROFILES.get(Platform.GREENGRASS)!;
    expect(p.transport).toBe(Transport.IPC);
    expect(p.configSource).toBe("GG_CONFIG");
  });
});
