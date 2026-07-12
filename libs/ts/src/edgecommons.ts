/**
 * EdgeCommons (TypeScript) — library entry point / lifecycle.
 *
 * {@link EdgeCommonsBuilder} parses the standard CLI contract, initializes messaging
 * for the runtime mode, loads + validates configuration from the selected source,
 * initializes logging, metrics, and the heartbeat, and wires config hot-reload.
 * Mirrors the Rust `EdgeCommonsBuilder` / `EdgeCommons`.
 *
 * TypeScript has no RAII/Drop, so resources are released by {@link EdgeCommons.close}
 * (stops the heartbeat + config watch and disconnects messaging) rather than on GC.
 */
import { parseArgs, ParsedArgs } from "./cli";
import {
  Transport,
  profileLoggingFormat,
  profileHealthEnabled,
  profileMetricTarget,
  profileMetricLogPath,
  profileCredentialsKeyProvider,
} from "./platform";
import { Config } from "./config/model";
import { HealthServer, ReadinessState } from "./health";
import { resolve } from "./config/template";
import { validate } from "./config/validation";
import { buildConfigSource, ConfigSource, ConfigWatch } from "./config/source";
import { LayeredConfigCoordinator } from "./config/layered";
import type { JsonObject } from "./config/merge";
import {
  ConfigurationChangeListener,
  ConfigurationValidationPhase,
  ConfigurationValidationResult,
  ConfigurationValidator,
} from "./config";
import { EffectiveConfigPublisher, redact } from "./config/effective_config";
import { CommandInbox, CommandInboxState } from "./commands";
import { EdgeCommonsError } from "./errors";
import { AppFacade, DataFacade, EventsFacade } from "./facades";
import type { StreamSink } from "./facades";
import { Heartbeat } from "./heartbeat";
import type { InstanceConnectivityProvider } from "./instance_connectivity";
import { initLogging, reconfigureLogging, LoggingReconfigurer, logger } from "./logging";
import { LogBusService, LogService } from "./log_bus";
import { MessageBuilder, MessageIdentity } from "./message";
import { DefaultMessagingService } from "./messaging/service";
import { IMessagingService } from "./messaging/types";
import { StandaloneMqttProvider } from "./messaging/standalone-provider";
import { IpcMessagingProvider } from "./messaging/ipc-provider";
import { loadMessagingConfig, qosConfigFromBrokers } from "./messaging/config";
import { MetricEmitter } from "./metrics/service";
import { MetricService } from "./metrics/types";
import { RepublishListener } from "./republish_listener";
import { Uns, checkToken } from "./uns";
import type { StreamMetricsBridge, StreamService } from "./streaming";
import type { CredentialMetricsBridge, CredentialService } from "./credentials";
import type { ParameterService } from "./parameters";

/**
 * The per-instance seam (UNS-CANONICAL-DESIGN §3, D-U3): an instance-scoped handle whose only
 * job is to pre-bind the instance token into (a) the {@link Uns} topic builder, (b) the
 * {@link MessageBuilder}, and (c) the app-usable publish facades (`data()`/`events()`/`app()` —
 * DESIGN-class-facades §3). The messaging service stays instance-agnostic — `publish(topic,
 * msg)` already receives both the topic (minted by this handle's instance-bound `uns()`) and
 * the envelope (stamped by its instance-bound builder).
 *
 * Obtain handles from {@link EdgeCommons.instance} (validated + cached per id). Component-level
 * messages (everything not built through a handle) default to instance
 * {@link MessageIdentity.DEFAULT_INSTANCE}.
 */
export class EdgeCommonsInstance {
  private readonly unsValue: Uns;

  /** Lazily-created facades (per-instance; the facades hold no per-instance client state). */
  private dataFacade?: DataFacade;
  private eventsFacade?: EventsFacade;
  private appFacade?: AppFacade;

  /**
   * @internal Created by {@link EdgeCommons.instance}, which validates + caches per id.
   *
   * @param idValue         the instance token
   * @param configProvider  a snapshot accessor (envelope identity + `publish.channel` lookup)
   * @param componentIdentity the resolved component identity (pre-instance-binding)
   * @param includeRoot     the resolved `topic.includeRoot` mode
   * @param messagingProvider the (guarded) messaging service accessor, or `undefined` when
   *                          messaging is not available in this runtime mode (the facades then
   *                          throw `EdgeCommonsError.messaging` on first use, matching {@link EdgeCommons.messaging})
   * @param streamSinkProvider the stream seam for `data().via(stream)` (`undefined` when
   *                            streaming is not configured — a stream route then falls back to local)
   * @param clockMillis     the clock for the facades' time defaults (injected for deterministic tests)
   */
  constructor(
    private readonly idValue: string,
    private readonly configProvider: () => Config,
    componentIdentity: MessageIdentity,
    includeRoot: boolean,
    private readonly messagingProvider: () => IMessagingService | undefined = () => undefined,
    private readonly streamSinkProvider: () => StreamSink | undefined = () => undefined,
    private readonly clockMillis: () => number = () => Date.now(),
  ) {
    this.unsValue = new Uns(componentIdentity.withInstance(idValue), includeRoot);
  }

  /** This handle's instance token. */
  id(): string {
    return this.idValue;
  }

  /** The topic builder bound to this instance (topics minted with this instance token). */
  uns(): Uns {
    return this.unsValue;
  }

  /**
   * Starts a message pre-bound to this instance — equivalent to
   * `MessageBuilder.create(name, version).withConfig(gg.config()).withInstance(id())`, so
   * `build()` stamps the component identity with this handle's instance token.
   */
  newMessage(name: string, version: string): MessageBuilder {
    return MessageBuilder.create(name, version).withConfig(this.configProvider()).withInstance(this.idValue);
  }

  /** The (guarded) messaging service, or throw if none was wired in this runtime mode. */
  private requireMessaging(): IMessagingService {
    const messaging = this.messagingProvider();
    if (!messaging) {
      throw EdgeCommonsError.messaging("messaging is not available in this runtime mode");
    }
    return messaging;
  }

  /**
   * The `data()` publish facade bound to this instance (DESIGN-class-facades §2.1): builds +
   * validates the `SouthboundSignalUpdate` body (quality → GOOD, `serverTs` → now, samples
   * wrapper), sanitizes the signal path into the `data` channel, and routes on the resolved
   * channel (per-call ▸ config `publish.channel` ▸ LOCAL).
   */
  data(): DataFacade {
    if (this.dataFacade === undefined) {
      this.dataFacade = new DataFacade(
        this.configProvider,
        this.idValue,
        this.unsValue,
        this.requireMessaging(),
        this.streamSinkProvider(),
        this.clockMillis,
      );
    }
    return this.dataFacade;
  }

  /**
   * The `events()` publish facade bound to this instance (DESIGN-class-facades §2.2): operator
   * events & alarms on the `evt` class, deriving the `evt/{severity}/{type}` channel from the body.
   */
  events(): EventsFacade {
    if (this.eventsFacade === undefined) {
      this.eventsFacade = new EventsFacade(
        this.configProvider,
        this.idValue,
        this.unsValue,
        this.requireMessaging(),
        this.clockMillis,
      );
    }
    return this.eventsFacade;
  }

  /**
   * The `app()` publish facade bound to this instance (DESIGN-class-facades §2.3): free-form
   * inter-component pub/sub on the `app` class (named header + verbatim body).
   */
  app(): AppFacade {
    if (this.appFacade === undefined) {
      this.appFacade = new AppFacade(this.configProvider, this.idValue, this.unsValue, this.requireMessaging());
    }
    return this.appFacade;
  }
}

/** The initialized component runtime: wired services + the current config snapshot. */
export class EdgeCommons {
  constructor(
    private readonly componentNameValue: string,
    private readonly argsValue: ParsedArgs,
    private current: Config,
    private readonly messagingService: IMessagingService | undefined,
    private readonly metricsService: MetricService,
    private readonly logService: LogBusService,
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
  /**
   * The `_bcast` republish listener (DESIGN-uns §9.3/§9.4): the late-join lever that
   * re-announces the state keepalive / effective config on a reconnect-rehydration broadcast.
   * Always wired when messaging is available (no config surface); undefined otherwise.
   */
  private republishListener?: RepublishListener;
  /**
   * The library-owned command inbox — the minimal `commands()` facade (DESIGN-uns §9.5, slice
   * S2): subscribes `ecv1/{device}/{component}/main/cmd/#` on the primary connection,
   * dispatches `cmd` envelopes by verb (built-ins `ping` / `reload-config` /
   * `get-configuration` + custom registrations via {@link commands}), and replies to
   * `header.reply_to`. Always wired when messaging is available (no config surface); undefined
   * otherwise.
   */
  private commandInbox?: CommandInbox;
  /**
   * The component-identity-bound UNS topic builder (instance
   * {@link MessageIdentity.DEFAULT_INSTANCE}), lazily bound on first {@link uns} from the
   * initial config's resolved identity + `topic.includeRoot` (both fixed at startup, like the
   * Java facade).
   */
  private unsValue?: Uns;
  /** Cached per-id instance handles (UNS-CANONICAL-DESIGN §3, D-U3). */
  private readonly instanceHandles = new Map<string, EdgeCommonsInstance>();
  /**
   * The clock the per-instance publish facades (`data()`/`events()`) use for their time defaults
   * (`serverTs`/`timestamp` → now). System clock in production; the facades take an injected
   * `ClockMillis` directly in their own unit tests for determinism.
   */
  private readonly clockMillis: () => number = () => Date.now();

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

  /**
   * Registers the component's per-instance connectivity provider — the overridable surface for
   * reporting connectivity AT THE INSTANCE LEVEL (each configured connection's health) in the
   * `main` `state` keepalive's `instances` array, without minting a separate UNS instance per
   * connection (data + lifecycle stay under `main`; the #1c model). A reference adapter maps each
   * connection to its reachability: OPC UA server session / Modbus slave / file-replicator source
   * directory. Pass `undefined` to stop reporting.
   */
  setInstanceConnectivityProvider(provider: InstanceConnectivityProvider | undefined): void {
    this.heartbeat.setInstanceConnectivityProvider(provider);
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

  /** @internal Attach the `_bcast` republish listener after construction. */
  _setRepublishListener(listener: RepublishListener | undefined): void {
    this.republishListener = listener;
  }

  /** @internal Attach the command inbox after construction. */
  _setCommandInbox(inbox: CommandInbox | undefined): void {
    this.commandInbox = inbox;
  }

  /**
   * The command-inbox facade — the minimal `gg.commands()` surface (DESIGN-uns §9.5): register
   * custom command verbs with `commands().register(verb, handler)`; the built-in verbs (`ping`,
   * `reload-config`, `get-configuration`) are registered by the library and cannot be shadowed.
   * `undefined` when no messaging is available in this runtime mode (mirrors the Java
   * `getCommands()`, which may be `null` on a mock/subclass bring-up).
   */
  commands(): CommandInbox | undefined {
    return this.commandInbox;
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
      throw EdgeCommonsError.messaging("messaging is not available in this runtime mode");
    }
    return this.messagingService;
  }

  /** The metric service. */
  metrics(): MetricService {
    return this.metricsService;
  }

  /** The structured log publisher for the library-owned UNS `log` class. */
  logs(): LogService {
    return this.logService;
  }

  /**
   * The UNS topic builder + validator bound to this component's resolved identity (instance
   * `"main"`) and its `topic.includeRoot` setting (UNS-CANONICAL-DESIGN §2). For
   * instance-scoped topics use {@link instance}`.uns()`.
   */
  uns(): Uns {
    if (this.unsValue === undefined) {
      this.unsValue = new Uns(this.current.componentIdentity, this.current.topicIncludeRoot);
    }
    return this.unsValue;
  }

  /**
   * The instance-scoped handle for an instance token (UNS-CANONICAL-DESIGN §3, D-U3): a
   * {@link EdgeCommonsInstance} whose `uns()` mints topics with — and whose `newMessage(...)` stamps
   * envelopes with — this instance token. The token is validated against the §2.2 token rule;
   * handles are cached per id, so repeated calls return the same object. The id is
   * deliberately NOT verified against the configured `component.instances[]` (instances may be
   * created dynamically) — an unknown id is only logged at DEBUG as a diagnostic aid.
   *
   * @throws UnsValidationError when the token violates the §2.2 token rule
   */
  instance(instanceId: string): EdgeCommonsInstance {
    checkToken(instanceId, "instance id");
    let handle = this.instanceHandles.get(instanceId);
    if (handle === undefined) {
      const configured = this.current.instanceIds();
      if (!configured.includes(instanceId)) {
        logger.debug(
          `instance('${instanceId}'): id is not among the configured component.instances[] ids` +
            ` [${configured.join(", ")}] - creating a dynamic instance handle`,
        );
      }
      handle = new EdgeCommonsInstance(
        instanceId,
        () => this.current,
        this.current.componentIdentity,
        this.current.topicIncludeRoot,
        () => this.messagingService,
        () => this.streamSink(),
        this.clockMillis,
      );
      this.instanceHandles.set(instanceId, handle);
    }
    return handle;
  }

  /**
   * The stream seam the `data()` facade composes for a `stream:<name>` channel
   * (DESIGN-class-facades §4): binds `streams().stream(name).append(...)` when streaming is
   * configured, else `undefined` so the facade falls a stream route back to a LOCAL publish.
   */
  private streamSink(): StreamSink | undefined {
    const streams = this.streamsService;
    if (!streams) return undefined;
    return (name, partitionKey, timestampMs, payload) => streams.stream(name).append(partitionKey, timestampMs, payload);
  }

  /**
   * The `data()` publish facade for the component's `main` instance — the single-instance-
   * component convenience, equivalent to `instance("main").data()` (DESIGN-class-facades §3, D6).
   * Builds/validates the `SouthboundSignalUpdate` body.
   */
  data(): DataFacade {
    return this.instance(MessageIdentity.DEFAULT_INSTANCE).data();
  }

  /**
   * The `events()` publish facade for the component's `main` instance — equivalent to
   * `instance("main").events()` (DESIGN-class-facades §3, D6). Operator events & alarms on the
   * `evt` class.
   */
  events(): EventsFacade {
    return this.instance(MessageIdentity.DEFAULT_INSTANCE).events();
  }

  /**
   * The `app()` publish facade for the component's `main` instance — equivalent to
   * `instance("main").app()` (DESIGN-class-facades §3, D6). Free-form inter-component pub/sub on
   * the `app` class.
   */
  app(): AppFacade {
    return this.instance(MessageIdentity.DEFAULT_INSTANCE).app();
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
    // Unsubscribe the _bcast republish topics while messaging is still up (the
    // unsubscribe-before-exit rule) and stop reacting to republish broadcasts mid-teardown.
    await this.republishListener?.close().catch(() => undefined);
    // Unsubscribe the command inbox while messaging is still up (same rule) and stop
    // dispatching command verbs mid-teardown.
    await this.commandInbox?.close().catch(() => undefined);
    (this.parametersService as { close?: () => void } | undefined)?.close?.();
    this.credentialMetrics?.close();
    this.streamMetrics?.close();
    this.streamsService?.close();
    await this.logService.flush().catch(() => undefined);
    this.logService.close();
    // Stop the heartbeat BEFORE messaging disconnects so its best-effort STOPPED state
    // (UNS-CANONICAL-DESIGN §4.3 / D-U14) can still leave over the live transport.
    await this.heartbeat.stop().catch(() => undefined);
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
    this.unsValue = undefined;
    this.instanceHandles.clear();
  }
}

/** Fluent builder for {@link EdgeCommons} (the supported construction path). */
export class EdgeCommonsBuilder {
  private argv?: string[];
  private receiveOwn = true;
  private initialReadyValue = true;
  private validationTimeoutMs = 5_000;
  private readonly configurationValidators: Array<{ name: string; validator: ConfigurationValidator }> = [];
  private readonly commandConfigurators: Array<(inbox: CommandInbox) => void> = [];

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

  /** Set the initial application readiness gate (default: ready). */
  initialReady(value: boolean): this {
    this.initialReadyValue = value;
    return this;
  }

  /** Register a side-effect-free configuration candidate validator. */
  configurationValidator(name: string, validator: ConfigurationValidator): this {
    if (!/^[A-Za-z0-9_.-]{1,128}$/.test(name) || typeof validator !== "function") {
      throw EdgeCommonsError.config("configuration validator requires a safe non-empty name and function");
    }
    this.configurationValidators.push({ name, validator });
    return this;
  }

  /** Set the bounded per-validator deadline (1..300000 ms; default 5000 ms). */
  configurationValidationTimeout(timeoutMs: number): this {
    if (!Number.isInteger(timeoutMs) || timeoutMs < 1 || timeoutMs > 300_000) {
      throw EdgeCommonsError.config("configuration validation timeout must be an integer between 1 and 300000 ms");
    }
    this.validationTimeoutMs = timeoutMs;
    return this;
  }

  /** Register component command handlers before the inbox subscribes. */
  configureCommands(configure: (inbox: CommandInbox) => void): this {
    if (typeof configure !== "function") throw EdgeCommonsError.config("command configurator must be a function");
    this.commandConfigurators.push(configure);
    return this;
  }

  /** Parse args, load+validate config, init logging/messaging/metrics/heartbeat. */
  async build(): Promise<EdgeCommons> {
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
    const layeredConfig = new LayeredConfigCoordinator({
      source,
      sourceSpec: parsed.config,
      componentName: this.componentNameValue,
    });

    const effectiveRaw = await layeredConfig.loadEffective();
    validate(effectiveRaw);
    let current = Config.fromValue(this.componentNameValue, thingName, effectiveRaw);
    await runConfigurationValidators(
      this.configurationValidators,
      effectiveRaw,
      undefined,
      ConfigurationValidationPhase.Initial,
      this.validationTimeoutMs,
    );

    if (messaging instanceof DefaultMessagingService) {
      // UNS-CANONICAL-DESIGN §5 / D-U5: late-bind the request() default deadline from
      // messaging.requestTimeoutSeconds now that the config exists. Messaging is built BEFORE
      // config loads (the IPC/messaging-backed config sources need it), so until this bind the
      // built-in 30 s applied — deliberately, giving the CONFIG_COMPONENT bootstrap request a
      // deadline instead of hanging forever.
      messaging.setDefaultRequestTimeout(current.messagingRequestTimeoutMs());
      // §4.1 / D-U24: late-bind the reserved-class guard's topic.includeRoot flag the same way
      // (default false pre-bind - nothing publishes rooted topics pre-config). D-U27: bind the
      // EFFECTIVE root (includeRoot AND a multi-level hierarchy) so the guard's position-5
      // check agrees with topic-building, which no-ops includeRoot on a single-level hierarchy
      // (D-U25); otherwise a warned single-level+includeRoot misconfig would false-positive on
      // a legit app/evt/data channel whose first token is a reserved word.
      messaging.setGuardIncludeRoot(
        current.topicIncludeRoot && current.componentIdentity.hier.length >= 2,
      );
    }
    // Thread the resolved platform's default logging format into the configurator (Phase 1c / FR-LOG-1):
    // a KUBERNETES pod with no `logging.ts_format` logs structured stdout-JSON, while explicit config
    // still wins and HOST/GREENGRASS keep today's console/text default. The platform is known here
    // (resolved at parse time) even though config loads after the resolver/messaging.
    initLogging(current, { formatDefault: profileLoggingFormat(parsed.platform) });
    const logService = new LogBusService(() => current, messaging);
    // Deferred early-bootstrap observability: the resolver summary and the messaging "connected"
    // fact are produced BEFORE logging is configured, so they are emitted here — immediately after
    // initLogging — using values already resolved at parse time (`parsed`) and the messaging service.
    logger.info(
      `platform resolved: platform=${parsed.platform} transport=${parsed.transport} configSource=${parsed.config.kind} identity=${parsed.thing}`,
    );
    if (messaging?.connected()) {
      logger.info(`messaging connected (transport=${parsed.transport})`);
    }
    logger.info(
      `EdgeCommons initialized: component=${this.componentNameValue} thing=${thingName} configSource=${source.sourceName()}`,
    );

    // Thread the resolved platform's default metric target into the metrics service (Phase 1c /
    // FR-MET-4): a KUBERNETES pod with no `metricEmission.target` selects the pull-based prometheus
    // target, while explicit config still wins and HOST/GREENGRASS keep the library default (`log`).
    // Same threading as the logging-format/health defaults — no resolver→ConfigManager dependency.
    const emitter = await MetricEmitter.create(
      current,
      messaging,
      profileMetricTarget(parsed.platform),
      profileMetricLogPath(parsed.platform),
    );
    const metrics: MetricService = emitter;

    const listeners: ConfigurationChangeListener[] = [emitter, new LoggingReconfigurer(), logService];

    const heartbeat = Heartbeat.start(() => current, metrics, messaging);

    // Readiness state behind /readyz (FR-HB-1): messaging-connected (live getter; no wired service ⇒
    // not ready) AND the app's readyFlag (default true) AND not shutting down.
    const readiness = new ReadinessState(() => messaging?.connected() ?? false);
    readiness.setReady(this.initialReadyValue);

    // Build the runtime first so the reload closure can update its snapshot.
    let runtime: EdgeCommons;
    // The single validate/parse/apply/notify path for a reloaded EFFECTIVE config document, shared
    // by source hot reloads, base-layer hot reloads, and the `reload-config` command. The layered
    // coordinator performs raw-layer parsing/merge first; this path only accepts a fully merged
    // candidate and commits it before notifying listeners.
    const applyEffectiveConfig = async (rawUpdate: JsonObject): Promise<boolean> => {
      try {
        validate(rawUpdate);
      } catch (e) {
        logger.warn(`reloaded config failed validation; keeping previous: ${String(e)}`);
        return false;
      }
      let next: Config;
      try {
        next = Config.fromValue(this.componentNameValue, thingName, rawUpdate);
      } catch (e) {
        logger.warn(`reloaded config could not be parsed; keeping previous: ${String(e)}`);
        return false;
      }
      try {
        await runConfigurationValidators(
          this.configurationValidators,
          rawUpdate,
          current.raw,
          ConfigurationValidationPhase.Reload,
          this.validationTimeoutMs,
        );
      } catch (e) {
        logger.warn(`reloaded config rejected by application validator; keeping previous: ${stableConfigValidationError(e)}`);
        return false;
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
      return true;
    };

    runtime = new EdgeCommons(
      this.componentNameValue,
      parsed,
      current,
      messaging,
      metrics,
      logService,
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

    // §4.3: announce the effective (redacted) configuration on the UNS cfg topic - the startup
    // push; the publisher re-announces on every configuration change (it is registered as a
    // config-change listener). Best-effort (publishNow never throws).
    if (messaging) {
      const effectiveConfigPublisher = new EffectiveConfigPublisher(() => current, messaging);
      listeners.push(effectiveConfigPublisher);
      await effectiveConfigPublisher.publishNow();

      // §9.3/§9.4: subscribe the own-device _bcast republish topics on the primary connection
      // so the uns-bridge's reconnect-rehydration broadcast (and a console's explicit
      // republish) gets a jittered, coalesced state/cfg re-announce. Always on (no config
      // surface); best-effort start (a failure disables the listener only).
      const republishListener = new RepublishListener(
        () => current,
        messaging,
        () => heartbeat.publishStateNow(),
        () => effectiveConfigPublisher.publishNow(),
      );
      await republishListener.start();
      runtime._setRepublishListener(republishListener);

      // §9.5 (slice S2): subscribe the component's own command inbox
      // (ecv1/{device}/{component}/main/cmd/#) on the primary connection and dispatch cmd
      // envelopes by verb - built-ins ping / reload-config / get-configuration answer the
      // console out of the box; apps add custom verbs via gg.commands().register(). Always on
      // (no config surface); best-effort start (a failure disables the inbox only).
      // A component that uses the pre-start command configurator has declared that command
      // handlers are part of its serving contract (the camera adapter does). Preserve the
      // established readiness behavior for older components that only use the optional
      // post-build `commands()` facade, while still exposing their inbox failure state.
      const commandPlaneRequired = this.commandConfigurators.length > 0;
      const commandInbox = new CommandInbox(
        () => current,
        messaging,
        () => heartbeat.getUptimeSecs(),
        async () => layeredConfig.reloadFromProvider(applyEffectiveConfig),
        () => effectiveConfigPublisher.redactedEffectiveConfig(),
        (state) => {
          if (commandPlaneRequired) readiness.setDependenciesReady(state === CommandInboxState.Active);
        },
      );
      // A configured command plane is a startup/readiness gate. It becomes true only after all
      // handlers are installed and the exact transport filter is acknowledged.
      if (commandPlaneRequired) readiness.setDependenciesReady(false);
      try {
        for (const configure of this.commandConfigurators) configure(commandInbox);
      } catch (e) {
        commandInbox.failStartup();
        logger.warn("command inbox configuration failed; command plane is unavailable");
        logger.debug(`command inbox configuration detail: ${String(e)}`);
      }
      await commandInbox.start();
      runtime._setCommandInbox(commandInbox);
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
    runtime._setWatch(await layeredConfig.watch(applyEffectiveConfig));
    return runtime;
  }
}

/** Run validators against defensive snapshots without exposing a commit-capable config object. */
async function runConfigurationValidators(
  validators: ReadonlyArray<{ name: string; validator: ConfigurationValidator }>,
  candidate: JsonObject,
  current: JsonObject | undefined,
  phase: ConfigurationValidationPhase,
  timeoutMs: number,
): Promise<void> {
  for (const { name, validator } of validators) {
    const candidateView = freezeJson(cloneJson(candidate));
    const currentView = current === undefined ? undefined : freezeJson(redact(current));
    let timer: NodeJS.Timeout | undefined;
    try {
      const result = await Promise.race<ConfigurationValidationResult>([
        Promise.resolve().then(() => validator(candidateView, currentView, phase)),
        new Promise<ConfigurationValidationResult>((_resolve, reject) => {
          timer = setTimeout(
            () => reject(EdgeCommonsError.config(`CONFIGURATION_VALIDATOR_TIMEOUT: ${name}`)),
            timeoutMs,
          );
          if (typeof timer.unref === "function") timer.unref();
        }),
      ]);
      if (!result || result.accepted !== true) {
        const diagnostic = result && "error" in result ? stableValidatorDiagnostic(result.error) : "rejected";
        throw EdgeCommonsError.config(`CONFIGURATION_VALIDATOR_REJECTED: ${name}: ${diagnostic}`);
      }
    } catch (e) {
      if (e instanceof EdgeCommonsError) throw e;
      // Do not render arbitrary exception text: validators see config and could accidentally
      // include a credential in an exception. The validator's explicit reject channel above is
      // the safe diagnostic surface.
      throw EdgeCommonsError.config(`CONFIGURATION_VALIDATOR_FAILED: ${name}`);
    } finally {
      if (timer) clearTimeout(timer);
    }
  }
}

function stableValidatorDiagnostic(value: unknown): string {
  if (typeof value !== "string") return "rejected";
  const normalized = value.replace(/[\x00-\x1F\x7F]/g, " ").trim();
  return normalized.length === 0 ? "rejected" : normalized.slice(0, 256);
}

function cloneJson(value: JsonObject): JsonObject {
  return JSON.parse(JSON.stringify(value)) as JsonObject;
}

function freezeJson<T>(value: T): T {
  if (value && typeof value === "object") {
    for (const item of Object.values(value as Record<string, unknown>)) freezeJson(item);
    Object.freeze(value);
  }
  return value;
}

function stableConfigValidationError(error: unknown): string {
  // Errors produced by this module are already stable. Do not render arbitrary validator
  // exceptions, which may contain the candidate configuration.
  return error instanceof EdgeCommonsError ? error.message : "CONFIGURATION_VALIDATOR_FAILED";
}

/** Whether the resolved transport is Greengrass IPC. */
function transportIsIpc(transport: Transport): boolean {
  return transport === Transport.IPC;
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
      throw EdgeCommonsError.messaging(
        "MQTT transport requires a messaging-config path (--transport MQTT <messaging_config.json>)",
      );
    }
    const mc = await loadMessagingConfig(messagingConfigPath);
    const provider = await StandaloneMqttProvider.connect(mc);
    return { service: new DefaultMessagingService(provider, qosConfigFromBrokers(mc)) };
  }
  // IPC (GREENGRASS)
  const provider = await IpcMessagingProvider.connect({ receiveOwnMessages });
  return { service: new DefaultMessagingService(provider), ipcProvider: provider };
}
