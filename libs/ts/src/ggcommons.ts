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

  /** @internal Attach the config-watch handle after construction. */
  _setWatch(watch: ConfigWatch | undefined): void {
    this.configWatch = watch;
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
