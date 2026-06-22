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
import { parseArgs, ParsedArgs, RuntimeMode } from "./cli";
import { Config } from "./config/model";
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

/** Default thing name when none is supplied and not running under Greengrass. */
const DEFAULT_THING_NAME = "NOT_GREENGRASS";
/** Greengrass-injected environment variable for the core's thing name. */
const THING_NAME_ENV = "AWS_IOT_THING_NAME";

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
  ) {}

  private configWatch?: ConfigWatch;
  private streamsService?: StreamService;
  private streamMetrics?: StreamMetricsBridge;
  private credentialsService?: CredentialService;
  private credentialMetrics?: CredentialMetricsBridge;
  private parametersService?: ParameterService;

  /** @internal Attach the config-watch handle after construction. */
  _setWatch(watch: ConfigWatch | undefined): void {
    this.configWatch = watch;
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

  /** The messaging service, or throw if none was wired (GREENGRASS without IPC). */
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

  /** Release resources: stop the heartbeat + config watch and disconnect messaging. */
  async close(): Promise<void> {
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
  }

  /** @internal Apply a reloaded snapshot and notify listeners. */
  _applyReload(snapshot: Config): void {
    this.current = snapshot;
  }
}

/** Fluent builder for {@link GGCommons} (the supported construction path). */
export class GGCommonsBuilder {
  private argv?: string[];
  private receiveOwn = false;

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
   * Java/Python/Rust `receiveOwnMessages` flag; default `false` =
   * RECEIVE_MESSAGES_FROM_OTHERS). Honored in GREENGRASS mode (the IPC ReceiveMode);
   * in STANDALONE mode the local broker delivers per its own semantics.
   */
  receiveOwnMessages(value: boolean): this {
    this.receiveOwn = value;
    return this;
  }

  /** Parse args, load+validate config, init logging/messaging/metrics/heartbeat. */
  async build(): Promise<GGCommons> {
    const parsed = parseArgs(this.argv ?? process.argv.slice(2));
    const thingName = parsed.thing ?? process.env[THING_NAME_ENV] ?? DEFAULT_THING_NAME;

    // Messaging is initialized first (it depends only on the runtime mode), and the
    // CONFIG_COMPONENT / GG_CONFIG / SHADOW sources need a handle to fetch config.
    const { service: messaging, ipcProvider } = await initMessaging(parsed.mode, this.receiveOwn);

    const source = buildConfigSource(parsed.config, {
      messaging,
      ipcProvider,
      thingName,
      componentName: this.componentNameValue,
    });

    const raw = await source.load();
    validate(raw);
    let current = Config.fromValue(this.componentNameValue, thingName, raw);

    initLogging(current);
    logger.info(
      `GGCommons initialized: component=${this.componentNameValue} thing=${thingName} configSource=${source.sourceName()}`,
    );

    const emitter = await MetricEmitter.create(current, messaging);
    const metrics: MetricService = emitter;

    const listeners: ConfigurationChangeListener[] = [emitter, new LoggingReconfigurer()];

    const heartbeat = Heartbeat.start(() => current, metrics, messaging);

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
      credentialService = await credentialsApi.openFromConfig(resolved, namespace);
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

    // Attach the watch only after the runtime exists, so a reload that fires during
    // subscription setup has a valid runtime to update.
    runtime._setWatch(await source.watch(onUpdate));
    return runtime;
  }
}

/** Initialize the messaging service + IPC provider handle for the runtime mode. */
async function initMessaging(
  mode: RuntimeMode,
  receiveOwnMessages: boolean,
): Promise<{ service: IMessagingService | undefined; ipcProvider?: IpcMessagingProvider }> {
  if (mode.kind === "STANDALONE") {
    const mc = await loadMessagingConfig(mode.messagingConfigPath);
    const provider = await StandaloneMqttProvider.connect(mc);
    return { service: new DefaultMessagingService(provider) };
  }
  // GREENGRASS
  const provider = await IpcMessagingProvider.connect({ receiveOwnMessages });
  return { service: new DefaultMessagingService(provider), ipcProvider: provider };
}
