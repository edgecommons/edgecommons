/**
 * GGCommons (TypeScript) — library entry point / lifecycle.
 *
 * {@link GGCommonsBuilder} parses the standard CLI contract, initializes messaging
 * for the runtime mode, loads + validates configuration from the selected source,
 * initializes logging, metrics, and the heartbeat, and wires config hot-reload.
 * Mirrors the Rust `GgCommonsBuilder` / `GgCommons`.
 *
 * TypeScript has no RAII/Drop, so resources are released by {@link GGCommons.close}
 * (stops the heartbeat + config watch and disconnects messaging) rather than on GC.
 */
import { parseArgs, ParsedArgs } from "./cli";
import {
  Transport,
  profileLoggingFormat,
  profileHealthEnabled,
  profileMetricTarget,
  profileCredentialsKeyProvider,
} from "./platform";
import { Config } from "./config/model";
import { HealthServer, ReadinessState } from "./health";
import { resolve } from "./config/template";
import { validate } from "./config/validation";
import { buildConfigSource, ConfigSource, ConfigWatch } from "./config/source";
import { ConfigurationChangeListener } from "./config";
import { GgError } from "./errors";
import { Heartbeat } from "./heartbeat";
import { initLogging, reconfigureLogging, LoggingReconfigurer, logger } from "./logging";
import { DefaultMessagingService } from "./messaging/service";
import { IMessagingService } from "./messaging/types";
import { StandaloneMqttProvider } from "./messaging/standalone-provider";
import { IpcMessagingProvider } from "./messaging/ipc-provider";
import { loadMessagingConfig } from "./messaging/config";
import { MetricEmitter } from "./metrics/service";
import { MetricService } from "./metrics/types";
import type { StreamMetricsBridge, StreamService } from "./streaming";
import type { CredentialMetricsBridge, CredentialService } from "./credentials";
import type { ParameterService } from "./parameters";

/** The initialized component runtime: wired services + the current config snapshot. */
export class GGCommons {
  constructor(
    private readonly componentNameValue: string,
    private readonly argsValue: ParsedArgs,
    private current: Config,
    private readonly messagingService: IMessagingService | undefined,
    private readonly metricsService: MetricService,
    private readonly listeners: ConfigurationChangeListener[],
    private readonly heartbeat: Heartbeat,
    private readonly configSource: ConfigSource,
    private readonly readiness: ReadinessState,
  ) {}

  private configWatch?: ConfigWatch;
  private healthServer?: HealthServer;
  private signalHandlers: Array<[NodeJS.Signals, () => void]> = [];
  private closed = false;
  private streamsService?: StreamService;
  private streamMetrics?: StreamMetricsBridge;
  private credentialsService?: CredentialService;
  private credentialMetrics?: CredentialMetricsBridge;
  private parametersService?: ParameterService;

  /** @internal Attach the config-watch handle after construction. */
  _setWatch(watch: ConfigWatch | undefined): void {
    this.configWatch = watch;
  }

  /** @internal Attach the HTTP health server after construction (FR-HB-1). */
  _setHealth(server: HealthServer | undefined): void {
    this.healthServer = server;
  }

  /**
   * @internal Wire SIGTERM/SIGINT to graceful shutdown — the library owns this so a component never
   * leaks subscriptions on the shared connection (FR-HB-2). On signal: flip `/readyz` to 503
   * immediately, run the idempotent {@link close} path, then `process.exit(0)`. Handlers are removed in
   * {@link close} so a clean library shutdown leaves no listeners behind.
   */
  _installSignalHandlers(): void {
    for (const signal of ["SIGTERM", "SIGINT"] as NodeJS.Signals[]) {
      const handler = (): void => {
        this.readiness.beginShutdown();
        logger.info(`${signal} received; shutting down`);
        this.close()
          .catch((e) => logger.warn(`shutdown error: ${String(e)}`))
          .finally(() => process.exit(0));
      };
      process.on(signal, handler);
      this.signalHandlers.push([signal, handler]);
    }
  }

  /**
   * Set the app-controlled readiness flag consumed by `/readyz` and `/startupz` (FR-HB-1). Defaults to
   * `true`, so a component is ready as soon as messaging connects. Call `setReady(false)` early (before
   * subscribing to required topics) and `setReady(true)` once the component can serve, to gate traffic
   * on the component's own startup work. Mirrors the Rust/Java/Python `setReady`/`set_ready`.
   */
  setReady(ready: boolean): void {
    this.readiness.setReady(ready);
  }

  /** Whether the runtime is currently ready (`messaging connected && readyFlag && !shuttingDown`). */
  ready(): boolean {
    return this.readiness.isReady();
  }

  /** @internal Attach the streaming service + metrics bridge after construction. */
  _setStreaming(service: StreamService | undefined, bridge: StreamMetricsBridge | undefined): void {
    this.streamsService = service;
    this.streamMetrics = bridge;
  }

  /**
   * The telemetry streaming service, or `undefined` if the component config has no `streaming`
   * section. Obtain a stream with `service.stream(name)` and append durable records. Mirrors the
   * Rust/Java/Python `gg.streams()`.
   */
  streams(): StreamService | undefined {
    return this.streamsService;
  }

  /** @internal Attach the credential service after construction. */
  _setCredentials(service: CredentialService | undefined): void {
    this.credentialsService = service;
  }

  /** @internal Attach the credential metrics bridge after construction. */
  _setCredentialMetrics(bridge: CredentialMetricsBridge | undefined): void {
    this.credentialMetrics = bridge;
  }

  /**
   * The credential service, or `undefined` if the component config has no `credentials` section.
   * Mirrors the Rust `gg.credentials()` / Java/Python `getCredentials()`/`get_credentials()`.
   */
  credentials(): CredentialService | undefined {
    return this.credentialsService;
  }

  /** @internal Attach the parameter service after construction. */
  _setParameters(service: ParameterService | undefined): void {
    this.parametersService = service;
  }

  /**
   * The parameter service, or `undefined` if the component config has no `parameters` section.
   * Offline-first reads of externalized config (`get`/`getByPath`/typed accessors). Mirrors the
   * Rust `gg.parameters()`.
   */
  parameters(): ParameterService | undefined {
    return this.parametersService;
  }

  /** The component's full name. */
  componentName(): string {
    return this.componentNameValue;
  }

  /** The parsed standard CLI arguments. */
  args(): ParsedArgs {
    return this.argsValue;
  }

  /** A consistent snapshot of the current configuration (replaced atomically on reload). */
  config(): Config {
    return this.current;
  }

  /** The messaging service, or throw if none was wired. */
  messaging(): IMessagingService {
    if (!this.messagingService) {
      throw GgError.messaging("messaging is not available in this runtime mode");
    }
    return this.messagingService;
  }

  /** The metric service. */
  metrics(): MetricService {
    return this.metricsService;
  }

  /** Register a listener invoked after the configuration is hot-reloaded. */
  addConfigChangeListener(listener: ConfigurationChangeListener): void {
    this.listeners.push(listener);
  }

  /** Remove a previously-registered config-change listener (by identity). */
  removeConfigChangeListener(listener: ConfigurationChangeListener): void {
    const idx = this.listeners.indexOf(listener);
    if (idx >= 0) this.listeners.splice(idx, 1);
  }

  /**
   * Release resources: flip readiness to 503, unsubscribe/close every subsystem, and stop the health
   * server. Idempotent (safe to call from both the SIGTERM handler and the app). The shutting-down flag
   * is set first — even on a repeat call — so `/readyz` returns 503 the instant shutdown begins
   * (FR-HB-2), before the drain completes.
   */
  async close(): Promise<void> {
    this.readiness.beginShutdown();
    if (this.closed) return;
    this.closed = true;
    // Drop the library-owned SIGTERM/SIGINT handlers so a clean shutdown leaves no listeners.
    for (const [signal, handler] of this.signalHandlers) {
      process.removeListener(signal, handler);
    }
    this.signalHandlers = [];
    (this.parametersService as { close?: () => void } | undefined)?.close?.();
    this.credentialMetrics?.close();
    this.streamMetrics?.close();
    this.streamsService?.close();
    this.heartbeat.stop();
    if (this.configWatch) await this.configWatch.close().catch(() => undefined);
    await this.metricsService.shutdown().catch(() => undefined);
    if (this.messagingService instanceof DefaultMessagingService) {
      await this.messagingService.disconnect().catch(() => undefined);
    }
    // Stop the health server last so `/readyz` keeps serving 503 throughout the drain above.
    if (this.healthServer) await this.healthServer.stop().catch(() => undefined);
  }

  /** @internal Apply a reloaded snapshot and notify listeners. */
  _applyReload(snapshot: Config): void {
    this.current = snapshot;
  }
}

/** Fluent builder for {@link GGCommons} (the supported construction path). */
export class GGCommonsBuilder {
  private argv?: string[];
  private receiveOwn = true;

  constructor(private readonly componentNameValue: string) {}

  /**
   * Supply argv WITHOUT the node/script prefix (i.e. `process.argv.slice(2)`).
   * If not set, `process.argv.slice(2)` is used.
   */
  args(argv: string[]): this {
    this.argv = argv;
    return this;
  }

  /**
   * Whether the component receives messages it itself published (mirrors the
   * Java/Python/Rust `receiveOwnMessages` flag; default `true` =
   * RECEIVE_ALL_MESSAGES, the Java-canonical value — DESIGN-core §12 #2). Honored on the IPC
   * transport (the IPC ReceiveMode); on the MQTT transport the local broker delivers per its own
   * semantics.
   */
  receiveOwnMessages(value: boolean): this {
    this.receiveOwn = value;
    return this;
  }

  /** Parse args, load+validate config, init logging/messaging/metrics/heartbeat. */
  async build(): Promise<GGCommons> {
    const parsed = parseArgs(this.argv ?? process.argv.slice(2));
    // The resolver already applied the identity precedence (-t ▸ AWS_IOT_THING_NAME ▸ default).
    const thingName = parsed.thing;

    // Messaging is initialized first (it depends only on the resolved transport), and the
    // CONFIG_COMPONENT / GG_CONFIG / SHADOW sources need a handle to fetch config.
    const { service: messaging, ipcProvider } = await initMessaging(
      parsed.transport,
      parsed.messagingConfigPath,
      this.receiveOwn,
    );

    const source = buildConfigSource(parsed.config, {
      messaging,
      ipcProvider,
      thingName,
      componentName: this.componentNameValue,
    });

    const raw = await source.load();
    validate(raw);
    let current = Config.fromValue(this.componentNameValue, thingName, raw);

    // Thread the resolved platform's default logging format into the configurator (Phase 1c / FR-LOG-1):
    // a KUBERNETES pod with no `logging.ts_format` logs structured stdout-JSON, while explicit config
    // still wins and HOST/GREENGRASS keep today's console/text default. The platform is known here
    // (resolved at parse time) even though config loads after the resolver/messaging.
    initLogging(current, { formatDefault: profileLoggingFormat(parsed.platform) });
    logger.info(
      `GGCommons initialized: component=${this.componentNameValue} thing=${thingName} configSource=${source.sourceName()}`,
    );

    // Thread the resolved platform's default metric target into the metrics service (Phase 1c /
    // FR-MET-4): a KUBERNETES pod with no `metricEmission.target` selects the pull-based prometheus
    // target, while explicit config still wins and HOST/GREENGRASS keep the library default (`log`).
    // Same threading as the logging-format/health defaults — no resolver→ConfigManager dependency.
    const emitter = await MetricEmitter.create(current, messaging, profileMetricTarget(parsed.platform));
    const metrics: MetricService = emitter;

    const listeners: ConfigurationChangeListener[] = [emitter, new LoggingReconfigurer()];

    const heartbeat = Heartbeat.start(() => current, metrics, messaging);

    // Readiness state behind /readyz (FR-HB-1): messaging-connected (live getter; no wired service ⇒
    // not ready) AND the app's readyFlag (default true) AND not shutting down.
    const readiness = new ReadinessState(() => messaging?.connected() ?? false);

    // Build the runtime first so the reload closure can update its snapshot.
    let runtime: GGCommons;
    const onUpdate = (rawUpdate: unknown): void => {
      try {
        validate(rawUpdate);
      } catch (e) {
        logger.warn(`reloaded config failed validation; keeping previous: ${String(e)}`);
        return;
      }
      let next: Config;
      try {
        next = Config.fromValue(this.componentNameValue, thingName, rawUpdate);
      } catch (e) {
        logger.warn(`reloaded config could not be parsed; keeping previous: ${String(e)}`);
        return;
      }
      current = next;
      runtime._applyReload(next);
      reconfigureLogging(next);
      logger.info("configuration reloaded");
      for (const listener of [...listeners]) {
        // Guard both synchronous throws and rejected promises so one bad listener
        // can never break a hot reload (matches the other libraries).
        try {
          Promise.resolve(listener.onConfigurationChange(next)).catch((e) =>
            logger.warn(`config change listener threw: ${String(e)}`),
          );
        } catch (e) {
          logger.warn(`config change listener threw: ${String(e)}`);
        }
      }
    };

    runtime = new GGCommons(
      this.componentNameValue,
      parsed,
      current,
      messaging,
      metrics,
      listeners,
      heartbeat,
      source,
      readiness,
    );
    // Credentials / local vault (only when a `credentials` config section is present). Loaded
    // dynamically so components that don't use it pay nothing. Opened BEFORE streaming so the vault
    // is available to resolve `$secret` references in the streaming config (mirrors Rust lib.rs).
    let credentialService: CredentialService | undefined;
    let credentialsApi: typeof import("./credentials") | undefined;
    const credentialsRaw = current.raw["credentials"];
    if (credentialsRaw && typeof credentialsRaw === "object") {
      credentialsApi = await import("./credentials");
      const resolved = JSON.parse(resolve(current, JSON.stringify(credentialsRaw)));
      // Transparently namespace every key by <thingName>/<componentName> (collision-free).
      const namespace = `${current.thingName}/${this.componentNameValue}`;
      // Platform-default vault key-provider (FR-CRED-6, precedence FR-RT-3): when `keyProvider.type`
      // is absent, KUBERNETES defaults to the env/software-KEK custodian; an explicit type wins, and
      // this does NOT auto-enable credentials (we only reach here because a section is present).
      credentialService = await credentialsApi.openFromConfig(
        resolved,
        namespace,
        profileCredentialsKeyProvider(parsed.platform),
      );
      runtime._setCredentials(credentialService);
      const credentialMetrics = new credentialsApi.CredentialMetricsBridge(current, metrics, credentialService);
      runtime._setCredentialMetrics(credentialMetrics);
      logger.info("Credentials vault initialized");
    }

    // Parameters (only when a `parameters` config section is present). Independent, offline-first
    // service for externalized config — sibling of credentials. Loaded dynamically so components
    // that don't use it pay nothing. No namespacing of parameter keys (matches the Rust port; the
    // cache path is per-component templated below).
    const parametersRaw = current.raw["parameters"];
    if (parametersRaw && typeof parametersRaw === "object") {
      const parametersApi = await import("./parameters");
      const resolved = JSON.parse(resolve(current, JSON.stringify(parametersRaw)));
      const parameterService = await parametersApi.openFromConfig(resolved);
      runtime._setParameters(parameterService);
      logger.info("Parameters service initialized");
    }

    // Telemetry streaming (only when a `streaming` config section is present, so components that
    // don't use it never load the native addon). Loaded dynamically for the same reason.
    const streamingRaw = current.raw["streaming"];
    if (streamingRaw && typeof streamingRaw === "object") {
      const streaming = await import("./streaming");
      // Resolve `$secret` references against the vault before streaming consumes its config, so
      // secrets never land in the templated/logged config snapshot (mirrors Rust §7).
      let streamingValue = JSON.parse(resolve(current, JSON.stringify(streamingRaw)));
      if (credentialService && credentialsApi) {
        streamingValue = credentialsApi.resolveSecretRefs(streamingValue, credentialService);
      }
      const streamingJson = JSON.stringify(streamingValue);
      const svc = streaming.StreamService.open(streamingJson);
      const names = streaming.StreamService.streamNames(streamingJson);
      const bridge = names.length
        ? new streaming.StreamMetricsBridge(current, metrics, svc, names)
        : undefined;
      runtime._setStreaming(svc, bridge);
      logger.info(`Telemetry streaming initialized with ${names.length} stream(s)`);
    }

    // HTTP health endpoint (FR-HB-1). Precedence (FR-RT-3): explicit `health.enabled` ▸ platform
    // default (on for KUBERNETES, off for GREENGRASS/HOST). The platform is known here (resolved at
    // parse time), reusing the same threading as the logging default — no resolver→ConfigManager dep.
    const healthCfg = current.parsed.health;
    const healthEnabled = healthCfg.enabled ?? profileHealthEnabled(parsed.platform);
    if (healthEnabled) {
      // A health-endpoint problem must NEVER crash the component (health is auxiliary), and by this
      // point messaging/heartbeat/streams are already live — letting a bind failure reject build()
      // would also leak them. Log and continue without a health server, mirroring Java/Rust.
      try {
        const health = await HealthServer.start({
          port: healthCfg.port,
          paths: {
            liveness: healthCfg.livenessPath,
            readiness: healthCfg.readinessPath,
            startup: healthCfg.startupPath,
          },
          readiness,
        });
        runtime._setHealth(health);
      } catch (e) {
        logger.error(`health server failed to start (continuing without it): ${String(e)}`);
      }
    }

    // The library owns SIGTERM/SIGINT → graceful shutdown (FR-HB-2): flip /readyz to 503, run the
    // idempotent close() (unsubscribe all + bounded-close), then exit 0. Components no longer wire
    // their own handlers (the example skeleton's duplicate is removed).
    runtime._installSignalHandlers();

    // Attach the watch only after the runtime exists, so a reload that fires during
    // subscription setup has a valid runtime to update.
    runtime._setWatch(await source.watch(onUpdate));
    return runtime;
  }
}

/**
 * Initialize the messaging service + IPC provider handle for the resolved transport (DESIGN-core
 * §4.2 transport-injection site). Branches on the resolved {@link Transport}, not a legacy mode enum.
 */
async function initMessaging(
  transport: Transport,
  messagingConfigPath: string | undefined,
  receiveOwnMessages: boolean,
): Promise<{ service: IMessagingService | undefined; ipcProvider?: IpcMessagingProvider }> {
  if (transport === Transport.MQTT) {
    if (!messagingConfigPath) {
      throw GgError.messaging(
        "MQTT transport requires a messaging-config path (--transport MQTT <messaging_config.json>)",
      );
    }
    const mc = await loadMessagingConfig(messagingConfigPath);
    const provider = await StandaloneMqttProvider.connect(mc);
    return { service: new DefaultMessagingService(provider) };
  }
  // IPC (GREENGRASS)
  const provider = await IpcMessagingProvider.connect({ receiveOwnMessages });
  return { service: new DefaultMessagingService(provider), ipcProvider: provider };
}
