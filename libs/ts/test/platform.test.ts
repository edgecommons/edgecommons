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
  ENV_KEY_PROVIDER,
  ENV_K8S_POD_NAME,
  ENV_K8S_SERVICE_HOST,
  ENV_K8S_THING_NAME,
  ENV_THING_NAME,
  JSON_LOG_FORMAT,
  K8S_SA_TOKEN_PATH,
  LOCAL_METRIC_LOG_PATH,
  PROFILES,
  PROMETHEUS_METRIC_TARGET,
  Platform,
  Transport,
  detectPlatform,
  profileCredentialsKeyProvider,
  profileLoggingFormat,
  profileHealthEnabled,
  profileMetricLogPath,
  profileMetricTarget,
  resolveIdentity,
  resolveProfile,
  validate,
} from "../src/platform";
import { resolve as resolveTemplate } from "../src/config/template";
import { Config } from "../src/config/model";
import { EdgeCommonsError } from "../src/errors";

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

  it("explicit HOST -> MQTT + FILE (Phase 1, §12 #1)", () => {
    const r = resolveProfile({ platform: Platform.HOST }, {});
    expect(r.platform).toBe(Platform.HOST);
    expect(r.transport).toBe(Transport.MQTT);
    expect(r.configSource).toEqual(["FILE"]);
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

  it("explicit KUBERNETES -> MQTT + CONFIGMAP (Phase 1a)", () => {
    const r = resolveProfile({ platform: Platform.KUBERNETES }, {});
    expect(r.platform).toBe(Platform.KUBERNETES);
    expect(r.transport).toBe(Transport.MQTT);
    expect(r.configSource).toEqual(["CONFIGMAP"]);
  });

  it("auto with a Kubernetes service-host env detects KUBERNETES -> MQTT + CONFIGMAP", () => {
    const r = resolveProfile({}, { [ENV_K8S_SERVICE_HOST]: "10.0.0.1" });
    expect(r.platform).toBe(Platform.KUBERNETES);
    expect(r.transport).toBe(Transport.MQTT);
    expect(r.configSource).toEqual(["CONFIGMAP"]);
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
  it("IPC on HOST fails the IPC lock", () => {
    expect(() => resolveProfile({ platform: Platform.HOST, transport: Transport.IPC }, {})).toThrow(
      /IPC transport requires --platform GREENGRASS/,
    );
  });

  it("IPC on KUBERNETES fails the IPC lock (the IPC×KUBERNETES rejection still holds)", () => {
    expect(() =>
      resolveProfile({ platform: Platform.KUBERNETES, transport: Transport.IPC }, {}),
    ).toThrow(/IPC transport requires --platform GREENGRASS/);
  });

  it("resolver failures are EdgeCommonsError of kind Cli", () => {
    try {
      resolveProfile({ platform: Platform.HOST, transport: Transport.IPC }, {});
      throw new Error("expected throw");
    } catch (e) {
      expect(e).toBeInstanceOf(EdgeCommonsError);
      expect((e as EdgeCommonsError).kind).toBe("Cli");
    }
  });
});

describe("validate", () => {
  it("rejects IPC on non-Greengrass", () => {
    expect(() => validate(Platform.HOST, Transport.IPC)).toThrow(EdgeCommonsError);
    expect(() => validate(Platform.KUBERNETES, Transport.IPC)).toThrow(EdgeCommonsError);
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

  it("treats a present-but-empty AWS_IOT_THING_NAME as absent -> NOT_GREENGRASS (cross-language parity)", () => {
    expect(resolveIdentity(undefined, Platform.HOST, { [ENV_THING_NAME]: "" })).toBe(
      DEFAULT_IDENTITY,
    );
  });

  it("explicit thing still wins over a present-but-empty env value", () => {
    expect(resolveIdentity("t1", Platform.HOST, { [ENV_THING_NAME]: "" })).toBe("t1");
  });

  it("handles an undefined env", () => {
    expect(resolveIdentity(undefined, Platform.HOST, undefined)).toBe(DEFAULT_IDENTITY);
  });
});

// ---------- FR-RT-7 / FR-CFG-6: Kubernetes Downward-API identity ----------

describe("resolveIdentity: KUBERNETES Downward-API (FR-RT-7)", () => {
  it("reads EDGECOMMONS_THING_NAME on KUBERNETES", () => {
    expect(
      resolveIdentity(undefined, Platform.KUBERNETES, { [ENV_K8S_THING_NAME]: "edge-42" }),
    ).toBe("edge-42");
  });

  it("falls back to POD_NAME on KUBERNETES when EDGECOMMONS_THING_NAME is absent", () => {
    expect(
      resolveIdentity(undefined, Platform.KUBERNETES, { [ENV_K8S_POD_NAME]: "ggc-pod-abc123" }),
    ).toBe("ggc-pod-abc123");
  });

  it("EDGECOMMONS_THING_NAME takes precedence over POD_NAME on KUBERNETES", () => {
    expect(
      resolveIdentity(undefined, Platform.KUBERNETES, {
        [ENV_K8S_THING_NAME]: "annotated-thing",
        [ENV_K8S_POD_NAME]: "ggc-pod-abc123",
      }),
    ).toBe("annotated-thing");
  });

  it("the KUBERNETES Downward-API tier wins over AWS_IOT_THING_NAME (only on KUBERNETES)", () => {
    expect(
      resolveIdentity(undefined, Platform.KUBERNETES, {
        [ENV_K8S_THING_NAME]: "k8s-thing",
        [ENV_THING_NAME]: "aws-thing",
      }),
    ).toBe("k8s-thing");
    // POD_NAME also wins over the AWS probe on KUBERNETES.
    expect(
      resolveIdentity(undefined, Platform.KUBERNETES, {
        [ENV_K8S_POD_NAME]: "k8s-pod",
        [ENV_THING_NAME]: "aws-thing",
      }),
    ).toBe("k8s-pod");
  });

  it("explicit -t/--thing still wins over every KUBERNETES env tier", () => {
    expect(
      resolveIdentity("explicit-thing", Platform.KUBERNETES, {
        [ENV_K8S_THING_NAME]: "k8s-thing",
        [ENV_K8S_POD_NAME]: "k8s-pod",
        [ENV_THING_NAME]: "aws-thing",
      }),
    ).toBe("explicit-thing");
  });

  it("KUBERNETES with only AWS_IOT_THING_NAME falls through to it (tier 3)", () => {
    expect(
      resolveIdentity(undefined, Platform.KUBERNETES, { [ENV_THING_NAME]: "aws-thing" }),
    ).toBe("aws-thing");
  });

  it("KUBERNETES with no identity env falls back to the default", () => {
    expect(resolveIdentity(undefined, Platform.KUBERNETES, {})).toBe(DEFAULT_IDENTITY);
  });

  it("present-but-empty k8s env vars are treated as absent", () => {
    expect(
      resolveIdentity(undefined, Platform.KUBERNETES, {
        [ENV_K8S_THING_NAME]: "",
        [ENV_K8S_POD_NAME]: "",
        [ENV_THING_NAME]: "aws-thing",
      }),
    ).toBe("aws-thing");
  });

  it("the KUBERNETES tier is NOT consulted on other platforms (HOST ignores EDGECOMMONS_THING_NAME/POD_NAME)", () => {
    // On HOST, the k8s Downward-API vars must be ignored; only AWS_IOT_THING_NAME / -t apply.
    expect(
      resolveIdentity(undefined, Platform.HOST, {
        [ENV_K8S_THING_NAME]: "k8s-thing",
        [ENV_K8S_POD_NAME]: "k8s-pod",
      }),
    ).toBe(DEFAULT_IDENTITY);
    expect(
      resolveIdentity(undefined, Platform.GREENGRASS, {
        [ENV_K8S_THING_NAME]: "k8s-thing",
        [ENV_THING_NAME]: "aws-thing",
      }),
    ).toBe("aws-thing");
  });

  it("the resolved KUBERNETES identity still passes template-variable sanitization", () => {
    // A hostile POD_NAME with path separators / wildcards / traversal must not break out of a
    // {ThingName}-interpolated path or topic (the resolved value is sanitized at interpolation).
    const identity = resolveIdentity(undefined, Platform.KUBERNETES, {
      [ENV_K8S_POD_NAME]: "../evil/+name#",
    });
    expect(identity).toBe("../evil/+name#");
    const cfg = Config.fromValue("com.example.Comp", identity, {});
    const out = resolveTemplate(cfg, "logs/{ThingName}/app.log");
    // The substituted value is sanitized: '/' and '\' separators, '+'/'#' wildcards, and '..'
    // traversal each collapse to '_'. The literal template separators are preserved.
    expect(out).toBe("logs/__evil__name_/app.log");
    expect(out).not.toContain("..");
    expect(out).not.toContain("+");
    expect(out).not.toContain("#");
  });
});

describe("profiles + enums", () => {
  it("PROFILES contains GREENGRASS, HOST, and KUBERNETES (Phase 1a)", () => {
    expect(PROFILES.size).toBe(3);
    expect(PROFILES.has(Platform.GREENGRASS)).toBe(true);
    expect(PROFILES.has(Platform.HOST)).toBe(true);
    expect(PROFILES.has(Platform.KUBERNETES)).toBe(true);
  });

  it("the KUBERNETES profile is MQTT + CONFIGMAP", () => {
    const p = PROFILES.get(Platform.KUBERNETES)!;
    expect(p.transport).toBe(Transport.MQTT);
    expect(p.configSource).toBe("CONFIGMAP");
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

describe("profile logging-format default (Phase 1c / FR-LOG-1)", () => {
  it("the KUBERNETES profile defaults its logging format to json", () => {
    expect(PROFILES.get(Platform.KUBERNETES)!.loggingFormat).toBe(JSON_LOG_FORMAT);
    expect(profileLoggingFormat(Platform.KUBERNETES)).toBe("json");
  });

  it("GREENGRASS/HOST pin no logging-format default (library default stays console/text)", () => {
    expect(PROFILES.get(Platform.GREENGRASS)!.loggingFormat).toBeUndefined();
    expect(PROFILES.get(Platform.HOST)!.loggingFormat).toBeUndefined();
    expect(profileLoggingFormat(Platform.GREENGRASS)).toBeUndefined();
    expect(profileLoggingFormat(Platform.HOST)).toBeUndefined();
  });
});

describe("profile health-endpoint default (Phase 1c / FR-HB-1)", () => {
  it("the KUBERNETES profile turns the health endpoint on by default", () => {
    expect(PROFILES.get(Platform.KUBERNETES)!.healthEnabled).toBe(true);
    expect(profileHealthEnabled(Platform.KUBERNETES)).toBe(true);
  });

  it("GREENGRASS/HOST leave the health endpoint off by default", () => {
    expect(PROFILES.get(Platform.GREENGRASS)!.healthEnabled).toBeUndefined();
    expect(PROFILES.get(Platform.HOST)!.healthEnabled).toBeUndefined();
    expect(profileHealthEnabled(Platform.GREENGRASS)).toBe(false);
    expect(profileHealthEnabled(Platform.HOST)).toBe(false);
  });
});

describe("profile metric-target default (Phase 1c prometheus / FR-MET-4)", () => {
  it("the KUBERNETES profile defaults its metric target to prometheus", () => {
    expect(PROFILES.get(Platform.KUBERNETES)!.metricTarget).toBe(PROMETHEUS_METRIC_TARGET);
    expect(profileMetricTarget(Platform.KUBERNETES)).toBe("prometheus");
  });

  it("GREENGRASS/HOST pin no metric-target default (library default stays log)", () => {
    expect(PROFILES.get(Platform.GREENGRASS)!.metricTarget).toBeUndefined();
    expect(PROFILES.get(Platform.HOST)!.metricTarget).toBeUndefined();
    expect(profileMetricTarget(Platform.GREENGRASS)).toBeUndefined();
    expect(profileMetricTarget(Platform.HOST)).toBeUndefined();
  });
});

describe("profile metric-log path default (HOST-aware default)", () => {
  it("HOST/KUBERNETES default the metric-log path to a local path", () => {
    expect(PROFILES.get(Platform.HOST)!.metricLogPath).toBe(LOCAL_METRIC_LOG_PATH);
    expect(PROFILES.get(Platform.KUBERNETES)!.metricLogPath).toBe(LOCAL_METRIC_LOG_PATH);
    expect(profileMetricLogPath(Platform.HOST)).toBe(LOCAL_METRIC_LOG_PATH);
    expect(profileMetricLogPath(Platform.KUBERNETES)).toBe(LOCAL_METRIC_LOG_PATH);
  });

  it("GREENGRASS pins no metric-log path default (library /greengrass default stays)", () => {
    expect(PROFILES.get(Platform.GREENGRASS)!.metricLogPath).toBeUndefined();
    expect(profileMetricLogPath(Platform.GREENGRASS)).toBeUndefined();
  });
});

describe("profile credentials key-provider default (Phase 1d / FR-CRED-6)", () => {
  it("the KUBERNETES profile defaults its vault key provider to env", () => {
    expect(PROFILES.get(Platform.KUBERNETES)!.credentialsKeyProvider).toBe(ENV_KEY_PROVIDER);
    expect(profileCredentialsKeyProvider(Platform.KUBERNETES)).toBe("env");
  });

  it("GREENGRASS/HOST pin no key-provider default (library default stays file)", () => {
    expect(PROFILES.get(Platform.GREENGRASS)!.credentialsKeyProvider).toBeUndefined();
    expect(PROFILES.get(Platform.HOST)!.credentialsKeyProvider).toBeUndefined();
    expect(profileCredentialsKeyProvider(Platform.GREENGRASS)).toBeUndefined();
    expect(profileCredentialsKeyProvider(Platform.HOST)).toBeUndefined();
  });
});
