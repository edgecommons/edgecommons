/**
 * Unit tests for the GGCommons lifecycle (`src/ggcommons.ts`) that don't need a live
 * broker or the native streaming addon.
 *
 * The existing `ggcommons_integration.test.ts` covers the happy STANDALONE+FILE path
 * against a real broker (and self-skips without one). This suite mocks the messaging
 * providers, the metric emitter, the heartbeat, and the dynamically-imported opt-in
 * subsystem barrels (credentials / parameters / streaming) so we can deterministically
 * exercise:
 *   - both transports (MQTT messaging-config load + IPC init),
 *   - every accessor (config/args/componentName/messaging/metrics) and the
 *     `messaging()` throw when no service is wired,
 *   - the opt-in subsystems wiring (and their accessors returning `undefined` when the
 *     matching config section is absent),
 *   - `close()` shutdown ordering across all subsystems.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";

// ---- mocked seams ----------------------------------------------------------

// Messaging providers: return inert handles; the DefaultMessagingService wrapping them
// is real, but we never publish through it here.
const standaloneConnect = vi.fn(async () => ({ kind: "standalone-provider", connected: () => true }));
const ipcConnect = vi.fn(async () => ({ kind: "ipc-provider", connected: () => true }));
const loadMessagingConfigMock = vi.fn(async () => ({ messaging: { local: { host: "h", port: 1 } } }));

// Health server: spy on start (keep ReadinessState + evaluateHealth real) so platform/config gating is
// asserted without binding a socket. start() resolves to a stub with a no-op stop().
const healthStop = vi.fn(async () => undefined);
const healthStart = vi.fn(async () => ({ stop: healthStop, port: () => 0 }));
vi.mock("../src/health", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../src/health")>();
  return { ...orig, HealthServer: { start: (...a: unknown[]) => healthStart(...a) } };
});

vi.mock("../src/messaging/standalone-provider", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../src/messaging/standalone-provider")>();
  return { ...orig, StandaloneMqttProvider: { connect: (...a: unknown[]) => standaloneConnect(...a) } };
});
vi.mock("../src/messaging/ipc-provider", () => ({
  IpcMessagingProvider: { connect: (...a: unknown[]) => ipcConnect(...a) },
}));
vi.mock("../src/messaging/config", () => ({ loadMessagingConfig: (...a: unknown[]) => loadMessagingConfigMock(...a) }));

// Metric emitter: a ConfigurationChangeListener + MetricService double.
const metricShutdown = vi.fn(async () => undefined);
vi.mock("../src/metrics/service", () => ({
  MetricEmitter: {
    create: vi.fn(async () => ({
      shutdown: metricShutdown,
      onConfigurationChange: vi.fn(async () => true),
      // minimal MetricService surface used by bridges/heartbeat (none called here)
      emit: vi.fn(),
    })),
  },
}));

// Heartbeat: capture the started instance so close() can be observed.
const heartbeatStop = vi.fn();
vi.mock("../src/heartbeat", () => ({
  Heartbeat: { start: vi.fn(() => ({ stop: heartbeatStop })) },
}));

// Opt-in subsystem barrels (loaded via dynamic import in build()).
const credBridgeClose = vi.fn();
const credOpen = vi.fn(async () => ({ kind: "cred-svc" }));
const resolveSecretRefsMock = vi.fn((v: unknown) => v);
vi.mock("../src/credentials", () => ({
  openFromConfig: (...a: unknown[]) => credOpen(...a),
  resolveSecretRefs: (...a: unknown[]) => resolveSecretRefsMock(...a),
  CredentialMetricsBridge: class {
    close = credBridgeClose;
  },
}));

const paramClose = vi.fn();
const paramOpen = vi.fn(async () => ({ kind: "param-svc", close: paramClose }));
vi.mock("../src/parameters", () => ({ openFromConfig: (...a: unknown[]) => paramOpen(...a) }));

const streamSvcClose = vi.fn();
const streamBridgeClose = vi.fn();
const streamOpen = vi.fn(() => ({ kind: "stream-svc", close: streamSvcClose }));
const streamNames = vi.fn(() => ["telemetry"]);
vi.mock("../src/streaming", () => ({
  StreamService: { open: (...a: unknown[]) => streamOpen(...a), streamNames: (...a: unknown[]) => streamNames(...a) },
  StreamMetricsBridge: class {
    close = streamBridgeClose;
  },
}));

// Import AFTER mocks are registered.
import { GGCommonsBuilder, GGCommons } from "../src/ggcommons";
import { GgError } from "../src/errors";

const BASE = { component: { global: {} }, logging: { level: "INFO" } };

beforeEach(() => {
  vi.clearAllMocks();
  standaloneConnect.mockResolvedValue({ kind: "standalone-provider", connected: () => true });
  ipcConnect.mockResolvedValue({ kind: "ipc-provider", connected: () => true });
  healthStart.mockResolvedValue({ stop: healthStop, port: () => 0 });
  loadMessagingConfigMock.mockResolvedValue({ messaging: { local: { host: "h", port: 1 } } });
  credOpen.mockResolvedValue({ kind: "cred-svc" });
  paramOpen.mockResolvedValue({ kind: "param-svc", close: paramClose });
  streamOpen.mockReturnValue({ kind: "stream-svc", close: streamSvcClose });
  streamNames.mockReturnValue(["telemetry"]);
  resolveSecretRefsMock.mockImplementation((v: unknown) => v);
});

afterEach(() => {
  vi.restoreAllMocks();
});

/** Build a GGCommons with the given config object (written nowhere — we mock ENV source). */
async function buildWith(
  cfg: Record<string, unknown>,
  extraArgs: string[] = ["--platform", "HOST", "--transport", "MQTT", "messaging.json"],
): Promise<GGCommons> {
  process.env.GGC_LIFECYCLE_CFG = JSON.stringify(cfg);
  const gg = await new GGCommonsBuilder("com.example.Lc")
    .args([...extraArgs, "-c", "ENV", "GGC_LIFECYCLE_CFG", "-t", "lc-thing"])
    .build();
  delete process.env.GGC_LIFECYCLE_CFG;
  return gg;
}

describe("GGCommons lifecycle (mocked)", () => {
  it("MQTT: loads messaging config, exposes accessors, opt-in subsystems undefined when absent", async () => {
    const gg = await buildWith(BASE);
    try {
      expect(loadMessagingConfigMock).toHaveBeenCalledWith("messaging.json");
      expect(standaloneConnect).toHaveBeenCalledTimes(1);
      expect(ipcConnect).not.toHaveBeenCalled();

      expect(gg.componentName()).toBe("com.example.Lc");
      expect(gg.args().thing).toBe("lc-thing");
      expect(gg.config().thingName).toBe("lc-thing");
      expect(gg.metrics()).toBeDefined();
      expect(gg.messaging()).toBeDefined();

      // No opt-in sections -> accessors return undefined.
      expect(gg.credentials()).toBeUndefined();
      expect(gg.parameters()).toBeUndefined();
      expect(gg.streams()).toBeUndefined();
    } finally {
      await gg.close();
    }
  });

  it("MQTT transport without a messaging-config path throws a messaging GgError at build", async () => {
    process.env.GGC_NO_MC = JSON.stringify(BASE);
    try {
      let err: unknown;
      try {
        await new GGCommonsBuilder("com.example.Lc")
          .args(["--platform", "HOST", "--transport", "MQTT", "-c", "ENV", "GGC_NO_MC", "-t", "lc-thing"])
          .build();
        throw new Error("expected build to throw");
      } catch (e) {
        err = e;
      }
      expect(err).toBeInstanceOf(GgError);
      expect((err as GgError).kind).toBe("Messaging");
      expect((err as GgError).message).toContain("MQTT transport requires a messaging-config path");
      // The path is enforced before we touch the messaging config / provider.
      expect(loadMessagingConfigMock).not.toHaveBeenCalled();
      expect(standaloneConnect).not.toHaveBeenCalled();
    } finally {
      delete process.env.GGC_NO_MC;
    }
  });

  it("IPC: initializes IPC messaging (no messaging-config file) honoring receiveOwnMessages", async () => {
    process.env.GGC_GG_CFG = JSON.stringify(BASE);
    const built = await new GGCommonsBuilder("com.example.Lc")
      .args(["--platform", "GREENGRASS", "-c", "ENV", "GGC_GG_CFG", "-t", "gg-thing"])
      .receiveOwnMessages(true)
      .build();
    delete process.env.GGC_GG_CFG;
    try {
      expect(ipcConnect).toHaveBeenCalledWith({ receiveOwnMessages: true });
      expect(loadMessagingConfigMock).not.toHaveBeenCalled();
      expect(built.messaging()).toBeDefined();
    } finally {
      await built.close();
    }
  });

  it("IPC: receiveOwnMessages defaults to true (Java-canonical convergence)", async () => {
    process.env.GGC_GG_CFG_DEF = JSON.stringify(BASE);
    const built = await new GGCommonsBuilder("com.example.Lc")
      .args(["--platform", "GREENGRASS", "-c", "ENV", "GGC_GG_CFG_DEF", "-t", "gg-thing"])
      .build();
    delete process.env.GGC_GG_CFG_DEF;
    try {
      expect(ipcConnect).toHaveBeenCalledWith({ receiveOwnMessages: true });
    } finally {
      await built.close();
    }
  });

  it("IPC: defaults thing name from AWS_IOT_THING_NAME / NOT_GREENGRASS when -t absent", async () => {
    process.env.GGC_GG_CFG2 = JSON.stringify(BASE);
    delete process.env.AWS_IOT_THING_NAME;
    const built = await new GGCommonsBuilder("com.example.Lc")
      .args(["--platform", "GREENGRASS", "-c", "ENV", "GGC_GG_CFG2"])
      .build();
    delete process.env.GGC_GG_CFG2;
    try {
      // No -t and no env -> DEFAULT_THING_NAME.
      expect(built.config().thingName).toBe("NOT_GREENGRASS");
    } finally {
      await built.close();
    }
  });

  it("messaging() throws GgError when no messaging service is wired", async () => {
    // Force IPC connect to yield no service by stubbing the provider connect to a value the
    // service still wraps — instead, simulate the "GREENGRASS without IPC" branch by directly
    // constructing GGCommons with an undefined messaging service via build then patching.
    const gg = await buildWith(BASE);
    try {
      // Replace the private messaging service with undefined to hit the throw branch.
      (gg as unknown as { messagingService: undefined }).messagingService = undefined;
      expect(() => gg.messaging()).toThrow(GgError);
      try {
        gg.messaging();
      } catch (e) {
        expect((e as GgError).kind).toBe("Messaging");
      }
    } finally {
      await gg.close();
    }
  });

  it("wires credentials when a `credentials` section is present and closes it", async () => {
    const gg = await buildWith({ ...BASE, credentials: { audit: { enabled: true } } });
    try {
      expect(credOpen).toHaveBeenCalledTimes(1);
      // Namespaced by <thing>/<component>.
      expect(credOpen.mock.calls[0][1]).toBe("lc-thing/com.example.Lc");
      expect(gg.credentials()).toEqual({ kind: "cred-svc" });
    } finally {
      await gg.close();
      expect(credBridgeClose).toHaveBeenCalled();
    }
  });

  it("threads NO key-provider default on HOST (3rd openFromConfig arg undefined → library default file)", async () => {
    const gg = await buildWith({ ...BASE, credentials: { audit: { enabled: true } } });
    try {
      // HOST profile pins no credentials key-provider default (FR-CRED-6).
      expect(credOpen.mock.calls[0][2]).toBeUndefined();
    } finally {
      await gg.close();
    }
  });

  it("threads the KUBERNETES key-provider default 'env' as the 3rd openFromConfig arg (FR-CRED-6)", async () => {
    const gg = await buildWith({ ...BASE, credentials: { audit: { enabled: true } } }, [
      "--platform",
      "KUBERNETES",
      "--transport",
      "MQTT",
      "messaging.json",
    ]);
    try {
      expect(credOpen).toHaveBeenCalledTimes(1);
      // KUBERNETES profile defaults the vault key provider to env; explicit keyProvider.type still wins
      // (precedence is enforced inside buildKeyProvider, which receives this default).
      expect(credOpen.mock.calls[0][2]).toBe("env");
    } finally {
      await gg.close();
    }
  });

  it("does NOT enable credentials on KUBERNETES when no `credentials` section is present", async () => {
    // The platform credentials default must never auto-enable the subsystem (it stays opt-in).
    const gg = await buildWith({ ...BASE }, ["--platform", "KUBERNETES", "--transport", "MQTT", "messaging.json"]);
    try {
      expect(credOpen).not.toHaveBeenCalled();
      expect(gg.credentials()).toBeUndefined();
    } finally {
      await gg.close();
    }
  });

  it("wires parameters when a `parameters` section is present and closes it", async () => {
    const gg = await buildWith({ ...BASE, parameters: { refreshIntervalSecs: 0 } });
    try {
      expect(paramOpen).toHaveBeenCalledTimes(1);
      expect(gg.parameters()).toMatchObject({ kind: "param-svc" });
    } finally {
      await gg.close();
      expect(paramClose).toHaveBeenCalled();
    }
  });

  it("wires streaming (with a metrics bridge) when streams are configured", async () => {
    const gg = await buildWith({ ...BASE, streaming: { streams: [{ name: "telemetry", sink: { type: "kinesis", streamName: "s" } }] } });
    try {
      expect(streamOpen).toHaveBeenCalledTimes(1);
      expect(streamNames).toHaveBeenCalledTimes(1);
      expect(gg.streams()).toMatchObject({ kind: "stream-svc" });
    } finally {
      await gg.close();
      expect(streamSvcClose).toHaveBeenCalled();
      expect(streamBridgeClose).toHaveBeenCalled();
    }
  });

  it("streaming with no stream names creates no metrics bridge", async () => {
    streamNames.mockReturnValue([]);
    const gg = await buildWith({ ...BASE, streaming: {} });
    try {
      expect(gg.streams()).toMatchObject({ kind: "stream-svc" });
    } finally {
      await gg.close();
      // Service still closed, but no bridge was created.
      expect(streamSvcClose).toHaveBeenCalled();
      expect(streamBridgeClose).not.toHaveBeenCalled();
    }
  });

  it("resolves `$secret` references in streaming config when credentials are present", async () => {
    const gg = await buildWith({
      ...BASE,
      credentials: { audit: { enabled: true } },
      streaming: { streams: [{ name: "telemetry", sink: { type: "kinesis", streamName: "s" } }] },
    });
    try {
      // Both credentials and streaming wired -> resolveSecretRefs invoked against the vault.
      expect(resolveSecretRefsMock).toHaveBeenCalledTimes(1);
      expect(resolveSecretRefsMock.mock.calls[0][1]).toEqual({ kind: "cred-svc" });
    } finally {
      await gg.close();
    }
  });

  it("add/removeConfigChangeListener mutate the listener list", async () => {
    const gg = await buildWith(BASE);
    try {
      const listener = { onConfigurationChange: () => true };
      gg.addConfigChangeListener(listener);
      // Removing a listener that was never added is a no-op (branch coverage).
      gg.removeConfigChangeListener({ onConfigurationChange: () => true });
      gg.removeConfigChangeListener(listener);
    } finally {
      await gg.close();
    }
  });

  it("close() stops heartbeat and shuts down metrics", async () => {
    const gg = await buildWith(BASE);
    await gg.close();
    expect(heartbeatStop).toHaveBeenCalledTimes(1);
    expect(metricShutdown).toHaveBeenCalledTimes(1);
  });

  it("build() rejects when config fails schema validation", async () => {
    await expect(
      buildWith({ component: { global: {} }, metricEmission: { target: "not-real" } }),
    ).rejects.toBeInstanceOf(GgError);
  });

  // ---- Phase 1c: health endpoint default-on-KUBERNETES + SIGTERM wiring (FR-HB-1/2) ----

  const K8S = ["--platform", "KUBERNETES", "--transport", "MQTT", "messaging.json"];

  it("health server is OFF by default on HOST", async () => {
    const gg = await buildWith(BASE);
    try {
      expect(healthStart).not.toHaveBeenCalled();
    } finally {
      await gg.close();
    }
  });

  it("health server is ON by default on KUBERNETES (default port 8081 + default paths)", async () => {
    const gg = await buildWith(BASE, K8S);
    try {
      expect(healthStart).toHaveBeenCalledTimes(1);
      const opts = healthStart.mock.calls[0][0] as {
        port: number;
        paths: { liveness: string; readiness: string; startup: string };
      };
      expect(opts.port).toBe(8081);
      expect(opts.paths).toEqual({ liveness: "/livez", readiness: "/readyz", startup: "/startupz" });
    } finally {
      await gg.close();
      expect(healthStop).toHaveBeenCalledTimes(1);
    }
  });

  it("explicit health.enabled=true turns it ON on HOST with custom port/paths", async () => {
    const gg = await buildWith({
      ...BASE,
      health: { enabled: true, port: 9111, livenessPath: "/l", readinessPath: "/r", startupPath: "/s" },
    });
    try {
      expect(healthStart).toHaveBeenCalledTimes(1);
      const opts = healthStart.mock.calls[0][0] as { port: number; paths: Record<string, string> };
      expect(opts.port).toBe(9111);
      expect(opts.paths).toEqual({ liveness: "/l", readiness: "/r", startup: "/s" });
    } finally {
      await gg.close();
    }
  });

  it("explicit health.enabled=false turns it OFF on KUBERNETES (explicit wins over the profile default)", async () => {
    const gg = await buildWith({ ...BASE, health: { enabled: false } }, K8S);
    try {
      expect(healthStart).not.toHaveBeenCalled();
    } finally {
      await gg.close();
    }
  });

  it("setReady() gates gg.ready(); default is ready once connected", async () => {
    const gg = await buildWith({ ...BASE, health: { enabled: true } });
    try {
      expect(gg.ready()).toBe(true); // connected (mock) + readyFlag default true + not shutting down
      gg.setReady(false);
      expect(gg.ready()).toBe(false);
      gg.setReady(true);
      expect(gg.ready()).toBe(true);
    } finally {
      await gg.close();
    }
  });

  it("close() flips readiness to not-ready, stops the health server, and is idempotent", async () => {
    const gg = await buildWith({ ...BASE, health: { enabled: true } });
    expect(gg.ready()).toBe(true);
    await gg.close();
    expect(gg.ready()).toBe(false); // shuttingDown -> 503
    expect(healthStop).toHaveBeenCalledTimes(1);
    expect(heartbeatStop).toHaveBeenCalledTimes(1);
    // Idempotent: a second close does not re-run the shutdown path.
    await gg.close();
    expect(healthStop).toHaveBeenCalledTimes(1);
    expect(heartbeatStop).toHaveBeenCalledTimes(1);
  });

  it("the library wires SIGTERM and removes the handler on close (FR-HB-2)", async () => {
    const before = process.listenerCount("SIGTERM");
    const gg = await buildWith(BASE);
    expect(process.listenerCount("SIGTERM")).toBe(before + 1);
    await gg.close();
    expect(process.listenerCount("SIGTERM")).toBe(before);
  });

  it("the SIGTERM handler flips readiness to 503 and exits 0 after a graceful close", async () => {
    const exitSpy = vi.spyOn(process, "exit").mockImplementation((() => undefined) as never);
    const before = process.listeners("SIGTERM").length;
    const gg = await buildWith({ ...BASE, health: { enabled: true } });
    try {
      const handlers = process.listeners("SIGTERM");
      // The library installed exactly one new SIGTERM handler; invoke ONLY ours (not vitest's own).
      expect(handlers.length).toBe(before + 1);
      const ours = handlers[handlers.length - 1] as () => void;

      expect(gg.ready()).toBe(true);
      ours();
      // beginShutdown() runs synchronously in the handler -> /readyz is 503 immediately.
      expect(gg.ready()).toBe(false);
      // Let the async close() drain, then assert it exited 0.
      await new Promise((r) => setImmediate(r));
      await new Promise((r) => setImmediate(r));
      expect(heartbeatStop).toHaveBeenCalled();
      expect(healthStop).toHaveBeenCalled();
      expect(exitSpy).toHaveBeenCalledWith(0);
    } finally {
      exitSpy.mockRestore();
      await gg.close();
    }
  });
});
