/**
 * <<COMPONENTNAME>> — application logic.
 *
 * Minimal starting point: holds the `ggcommons` service handles, registers a
 * configuration-change listener (dynamic config pickup), and runs until shutdown.
 * Replace the body of {@link App.run} with your component's business logic.
 */
import {
  Config,
  ConfigurationChangeListener,
  GGCommons,
  IMessagingService,
  MetricService,
  Uns,
  logger,
} from "@edgecommons/ggcommons";

/** The component's business logic and the `ggcommons` service handles it operates over. */
export class App {
  private readonly config: Config;
  private readonly metrics: MetricService;
  /**
   * The UNS topic builder bound to this component's config-resolved identity (the top-level
   * `hierarchy` + `identity` config blocks; the last hierarchy level's value is always the
   * resolved thing name). Mint every topic here — e.g.
   * `this.uns.topic(UnsClass.Data, "my-channel")` →
   * `ecv1/{device}/{component}/main/data/my-channel` — never hand-write topic strings.
   * Instance-scoped topics/messages come from `gg.instance(id).uns()` / `.newMessage(...)`.
   */
  private readonly uns: Uns;
  /** `undefined` when no messaging transport is available for the runtime mode. */
  private readonly messaging?: IMessagingService;

  constructor(gg: GGCommons) {
    this.config = gg.config();
    this.metrics = gg.metrics();
    this.uns = gg.uns();

    // Dynamic config pickup: react to deployment/shadow config changes at runtime.
    const listener: ConfigurationChangeListener = {
      onConfigurationChange: (config: Config): boolean => {
        logger.info(`configuration changed (thing=${config.thingName})`);
        return true;
      },
    };
    gg.addConfigChangeListener(listener);

    // gg.messaging() throws if no transport is wired (e.g. GREENGRASS without IPC),
    // so guard with try/catch and degrade to heartbeat-only when unavailable.
    try {
      this.messaging = gg.messaging();
    } catch {
      this.messaging = undefined;
    }
  }

  /** Run until {@link stop} is called. */
  async run(): Promise<void> {
    logger.info(`<<COMPONENTNAME>> running (thing=${this.config.thingName})`);

    // TODO: your business logic goes here. The wired services are available as:
    //   - this.messaging  — publish/subscribe + request/reply (may be undefined)
    //   - this.metrics    — this.metrics.defineMetric(..) / emitMetric(..)
    //   - this.config     — this.config.global() / this.config.thingName
    //   - this.uns        — mint topics: this.uns.topic(UnsClass.Data, "my-channel")
    //                       (import UnsClass from "@edgecommons/ggcommons")
    // The library heartbeat is automatic: a `state` keepalive publishes to
    // ecv1/{device}/{component}/main/state every heartbeat.intervalSecs (default 5 s) —
    // don't publish liveness yourself (`state` is a reserved class).
    // Touch the handles so the starting template compiles without warnings.
    void this.metrics;
    void this.messaging;
    void this.uns;
  }

  /** Stop the app and clean up before the runtime is closed. */
  async stop(): Promise<void> {
    // TODO: unsubscribe from topics and tear down any timers here.
  }
}
