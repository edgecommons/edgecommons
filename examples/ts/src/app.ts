/**
 * Skeleton application logic.
 *
 * {@link SkeletonApp} wires the concerns that every real component needs,
 * mirroring the Rust/Python/Java skeletons:
 * 1. **Request/reply** — subscribes to a request topic and replies to each request.
 * 2. **Periodic publish** — publishes a data message on an interval read from
 *    configuration (`component.global.publish_interval`), emitting a metric each time.
 * 3. **Dynamic config** — registers a `ConfigurationChangeListener` so the publish
 *    interval updates live on a config hot-reload, without a restart.
 * 4. **IoT Core telemetry** — mirrors each data message to AWS IoT Core.
 * 5. **Graceful shutdown** — runs until SIGINT / SIGTERM, then unsubscribes and lets
 *    `gg.close()` (in `main.ts`) release the runtime.
 *
 * Messaging is available in both STANDALONE and GREENGRASS mode. If it is
 * unavailable for the runtime mode, `gg.messaging()` throws — the app catches that
 * and degrades to heartbeat-only operation, simply waiting for shutdown.
 */
import {
  Config,
  ConfigurationChangeListener,
  GGCommons,
  IMessagingService,
  MessageBuilder,
  MetricBuilder,
  MetricService,
  Qos,
  logger,
} from "ggcommons";

/** Default publish interval (seconds) when `component.global.publish_interval` is absent. */
const DEFAULT_PUBLISH_INTERVAL_SECS = 3;
/** Subscription queue depth for the request topic. */
const REQUEST_QUEUE_SIZE = 16;
/** Handler concurrency for the request topic (`1` = serial, ordered). */
const REQUEST_CONCURRENCY = 1;
/** The metric emitted on each periodic publish. */
const PUBLISH_METRIC = "messages_published";

/**
 * The publish interval (seconds) from `component.global.publish_interval`, falling
 * back to {@link DEFAULT_PUBLISH_INTERVAL_SECS}. Greengrass stores configuration
 * numbers as doubles, so a value like `5` may come back as `5.0` — accept either.
 */
function intervalFrom(config: Config): number {
  const global = config.global();
  if (global && typeof global === "object") {
    const value = (global as Record<string, unknown>).publish_interval;
    if (typeof value === "number" && Number.isFinite(value) && value >= 1) {
      return Math.trunc(value);
    }
  }
  return DEFAULT_PUBLISH_INTERVAL_SECS;
}

/** The component's business logic and the service handles it operates over. */
export class SkeletonApp {
  private readonly config: Config;
  private readonly metrics: MetricService;
  /** `undefined` only when no messaging transport is available for the runtime mode. */
  private readonly messaging?: IMessagingService;
  /** Live publish interval (seconds), updated by the config-change listener on reload. */
  private publishInterval: number;

  private requestTopic?: string;
  private cmdTopic?: string;
  private publishTimer?: NodeJS.Timeout;
  private seq = 0;
  private running = false;

  constructor(gg: GGCommons) {
    this.config = gg.config();
    this.metrics = gg.metrics();

    // Define the metric emitted on each periodic publish.
    this.metrics.defineMetric(
      MetricBuilder.create(PUBLISH_METRIC).addMeasure("count", "Count", 60).build(),
    );

    this.publishInterval = intervalFrom(this.config);

    // Register for config hot-reload so the publish cadence tracks
    // `component.global.publish_interval` without a restart — the TypeScript
    // counterpart of the Rust skeleton's IntervalListener.
    const listener: ConfigurationChangeListener = {
      onConfigurationChange: (config: Config): boolean => {
        this.publishInterval = intervalFrom(config);
        logger.info(`configuration changed; updated publish interval to ${this.publishInterval}s`);
        return true;
      },
    };
    gg.addConfigChangeListener(listener);

    // Messaging is available in STANDALONE mode (always) and GREENGRASS mode (IPC);
    // gg.messaging() throws if no transport is wired, so guard with try/catch.
    try {
      this.messaging = gg.messaging();
    } catch {
      this.messaging = undefined;
    }
  }

  /**
   * Run the component until {@link stop} is called.
   *
   * Starts the request responder and the periodic publisher (when messaging is
   * available). When messaging is unavailable, runs heartbeat-only.
   */
  async run(): Promise<void> {
    this.running = true;
    const thing = this.config.thingName;

    const messaging = this.messaging;
    if (!messaging) {
      logger.warn("messaging unavailable for this runtime mode; running heartbeat-only until shutdown");
      return;
    }

    this.requestTopic = `${thing}/skeleton/request`;
    this.cmdTopic = `${thing}/skeleton/cmd`;
    const dataTopic = `${thing}/skeleton/data`;
    const telemetryTopic = `${thing}/skeleton/telemetry`;

    // 1. Respond to requests on the request topic (local pub/sub).
    await messaging.subscribe(
      this.requestTopic,
      async (topic, msg) => {
        logger.info(`received request on ${topic}: ${msg.header.name}`);
        const reply = MessageBuilder.create("SkeletonReply", "1.0")
          .withConfig(this.config)
          .withPayload({ echo: msg.getBody(), ok: true })
          .build();
        try {
          await messaging.reply(msg, reply);
        } catch (e) {
          logger.warn(`failed to send reply: ${String(e)}`);
        }
      },
      REQUEST_QUEUE_SIZE,
      REQUEST_CONCURRENCY,
    );
    logger.info(`subscribed for requests on ${this.requestTopic}`);

    // 2. Subscribe to commands from AWS IoT Core (the IoT Core bridge); ack each one
    //    back to IoT Core (exercises subscribeToIotCore + publishToIotCore).
    await messaging.subscribeToIotCore(
      this.cmdTopic,
      async (topic, msg) => {
        logger.info(`received IoT Core command on ${topic}`);
        const ack = MessageBuilder.create("CmdAck", "1.0")
          .withConfig(this.config)
          .withPayload({ ack: msg.getBody() })
          .build();
        try {
          await messaging.publishToIotCore(telemetryTopic, ack, Qos.AtLeastOnce);
        } catch (e) {
          logger.warn(`failed to ack IoT Core command: ${String(e)}`);
        }
      },
      Qos.AtLeastOnce,
      REQUEST_QUEUE_SIZE,
      REQUEST_CONCURRENCY,
    );
    logger.info(`subscribed to IoT Core commands on ${this.cmdTopic}`);

    // 3. Start the periodic publisher. It reschedules itself each tick from the live
    //    publish interval, so a config hot-reload takes effect on the next tick.
    this.scheduleNextPublish(messaging, dataTopic, telemetryTopic);
  }

  /** Schedule the next periodic publish using the live publish interval. */
  private scheduleNextPublish(messaging: IMessagingService, dataTopic: string, telemetryTopic: string): void {
    if (!this.running) return;
    const intervalMs = Math.max(this.publishInterval, 1) * 1000;
    this.publishTimer = setTimeout(() => {
      void this.publishOnce(messaging, dataTopic, telemetryTopic).finally(() =>
        this.scheduleNextPublish(messaging, dataTopic, telemetryTopic),
      );
    }, intervalMs);
  }

  /**
   * Publish one data message, mirror it to AWS IoT Core, and emit the publish metric.
   * Demonstrates config-driven periodic publishing plus metric emission.
   */
  private async publishOnce(messaging: IMessagingService, dataTopic: string, telemetryTopic: string): Promise<void> {
    this.seq += 1;
    const msg = MessageBuilder.create("SkeletonData", "1.0")
      .withConfig(this.config)
      .withPayload({ seq: this.seq })
      .build();
    try {
      await messaging.publish(dataTopic, msg);
      // Also mirror to AWS IoT Core (exercises the IoT Core bridge / publishToIotCore).
      try {
        await messaging.publishToIotCore(telemetryTopic, msg, Qos.AtLeastOnce);
      } catch (e) {
        logger.warn(`failed to publish telemetry to IoT Core: ${String(e)}`);
      }
      logger.info(`published data message on ${dataTopic} (seq=${this.seq})`);
      await this.metrics.emitMetric(PUBLISH_METRIC, { count: 1 });
    } catch (e) {
      logger.warn(`failed to publish data message: ${String(e)}`);
    }
  }

  /** Stop the loops and clean up subscriptions before the runtime is closed. */
  async stop(): Promise<void> {
    this.running = false;
    if (this.publishTimer) {
      clearTimeout(this.publishTimer);
      this.publishTimer = undefined;
    }
    const messaging = this.messaging;
    if (!messaging) return;
    try {
      if (this.requestTopic) await messaging.unsubscribe(this.requestTopic);
      if (this.cmdTopic) await messaging.unsubscribeFromIotCore(this.cmdTopic);
    } catch (e) {
      logger.warn(`error while unsubscribing: ${String(e)}`);
    }
  }
}
