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
 * Messaging is available on both the HOST platform (MQTT) and the GREENGRASS platform (IPC).
 * If it is unavailable for the resolved platform, `gg.messaging()` throws — the app catches that
 * and degrades to heartbeat-only operation, simply waiting for shutdown.
 */
import {
  Config,
  ConfigurationChangeListener,
  CredentialService,
  GGCommons,
  IMessagingService,
  MessageBuilder,
  MetricBuilder,
  MetricService,
  ParameterService,
  Qos,
  StreamHandle,
  logger,
} from "@breissinger/ggcommons";

/** Default publish interval (seconds) when `component.global.publish_interval` is absent. */
const DEFAULT_PUBLISH_INTERVAL_SECS = 3;
/** Config key (under `component.global`) naming the secret the component reads. */
const DEMO_SECRET_KEY = "demo_secret";
/** Default secret name when `component.global.demo_secret` is absent. */
const DEFAULT_DEMO_SECRET = "skeleton/demo-secret";
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
  /** Durable `telemetry` stream handle, or `undefined` if the config has no `streaming` section. */
  private readonly stream?: StreamHandle;
  /**
   * The credential service, or `undefined` if the config has no `credentials` section.
   * Demonstrates encrypted-vault secret access (and, with a `central` config, sync from
   * AWS Secrets Manager over TES). Mirrors the Rust skeleton's `credentials` field.
   */
  private readonly credentials?: CredentialService;
  /**
   * The parameter service, or `undefined` if the config has no `parameters` section.
   * Demonstrates offline-first externalized-config reads via `gg.parameters()` (here from an
   * `env` source — no AWS). Mirrors the Rust skeleton's `parameters` field.
   */
  private readonly parameters?: ParameterService;

  private requestTopic?: string;
  private cmdTopic?: string;
  private cmdSubscribed = false;
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

    // Messaging is available on the HOST platform (MQTT, always) and GREENGRASS platform (IPC);
    // gg.messaging() throws if no transport is wired, so guard with try/catch.
    try {
      this.messaging = gg.messaging();
    } catch {
      this.messaging = undefined;
    }

    // Durable telemetry stream (undefined unless the config has a `streaming` section with a
    // stream named "telemetry"). The publish loop appends each data point; the library's export
    // engine drains it to the configured sink (Kinesis) independently.
    const streams = gg.streams();
    if (streams) {
      try {
        this.stream = streams.stream("telemetry");
        logger.info("telemetry streaming enabled (stream 'telemetry')");
      } catch (e) {
        logger.warn(`stream 'telemetry' unavailable; streaming disabled: ${String(e)}`);
      }
    }

    // Credential service (undefined unless the config has a `credentials` section). Used by
    // demonstrateCredentials() once at startup; mirrors the Rust skeleton's gg.credentials().
    this.credentials = gg.credentials();

    // Parameter service (undefined unless the config has a `parameters` section). Used by
    // demonstrateParameters() once at startup; mirrors the Rust skeleton's gg.parameters().
    this.parameters = gg.parameters();
  }

  /**
   * Demonstrate encrypted-vault secret access via `gg.credentials()`.
   *
   * Show the credential-service usage every real component needs: read a named secret from the
   * encrypted local vault and use it — without ever logging the value. Runs once at startup.
   *
   * In production the secret arrives via central sync (AWS Secrets Manager over TES, with a
   * `credentials.central` config) or out-of-band provisioning; here, so the example is
   * self-contained, we seed a demo value locally on first run if it is absent.
   *
   * Non-fatal: any vault error is logged and swallowed so the demo never takes the component down.
   */
  private demonstrateCredentials(): void {
    const creds = this.credentials;
    if (!creds) {
      logger.info("no `credentials` config section; secret access demo disabled");
      return;
    }

    try {
      // Secret name from `component.global.demo_secret`, defaulting to a self-seeded demo secret.
      const global = this.config.global();
      let name = DEFAULT_DEMO_SECRET;
      if (global && typeof global === "object") {
        const value = (global as Record<string, unknown>)[DEMO_SECRET_KEY];
        if (typeof value === "string" && value.length > 0) {
          name = value;
        }
      }

      // Seed a demo secret on first run (in production this comes from central sync/provisioning).
      if (!creds.exists(name)) {
        const demo = Buffer.from(
          JSON.stringify({ username: "svc-account", password: "demo-secret-value" }),
          "utf-8",
        );
        const version = creds.put(name, demo);
        logger.info(
          `seeded demo secret (production: provided via central sync / provisioning): ${name} version=${version}`,
        );
      }

      // Read it back and use it — logging only non-sensitive facts, never the value.
      const s = creds.get(name);
      if (!s) {
        logger.warn(`secret not found after seeding (unexpected): ${name}`);
        return;
      }
      logger.info(
        `credential access OK (value redacted): ${name} bytes=${s.bytes().length} source=${s.source}`,
      );

      // A real component would now use the secret (e.g. authenticate a downstream client).
      // Demonstrate a typed view; log only the non-secret username.
      const ba = creds.getBasicAuth(name);
      if (ba) {
        logger.info(`parsed basic-auth view (password redacted): ${name} username=${ba.username}`);
      }
    } catch (e) {
      logger.warn(`credential demo failed (non-fatal): ${String(e)}`);
    }
  }

  /**
   * Demonstrate externalized-config reads via `gg.parameters()`.
   *
   * Show the parameter-service usage a real component needs: read named parameters from the
   * offline-first cache — including a typed (`getInt`) read. The example config wires an `env`
   * source (no AWS, no provisioning), so the values come from environment variables
   * (`GG_PARAM_SKELETON_REGION`, `GG_PARAM_SKELETON_POOLSIZE`). Runs once at startup.
   *
   * Logs only non-secret values; never logs a secure value. Non-fatal: any error is logged and
   * swallowed so the demo never takes the component down.
   */
  private demonstrateParameters(): void {
    const params = this.parameters;
    if (!params) {
      logger.info("no `parameters` config section; parameter access demo disabled");
      return;
    }

    try {
      // A plain string parameter (non-secret) — safe to log.
      const region = params.get("/skeleton/region");
      if (region !== undefined) {
        logger.info(`parameter access OK: /skeleton/region=${region}`);
      } else {
        logger.info("parameter /skeleton/region not set (set GG_PARAM_SKELETON_REGION to demo it)");
      }

      // A typed integer parameter — getInt parses + validates.
      const poolSize = params.getInt("/skeleton/poolSize");
      if (poolSize !== undefined) {
        logger.info(`parameter access OK (typed): /skeleton/poolSize=${poolSize}`);
      } else {
        logger.info("parameter /skeleton/poolSize not set (set GG_PARAM_SKELETON_POOLSIZE to demo it)");
      }

      const stats = params.stats();
      logger.info(`parameter store: source=${stats.source} count=${stats.parameterCount}`);
    } catch (e) {
      logger.warn(`parameter demo failed (non-fatal): ${String(e)}`);
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

    // Demonstrate encrypted-vault secret access once at startup (non-fatal).
    this.demonstrateCredentials();

    // Demonstrate externalized-config parameter access once at startup (non-fatal).
    this.demonstrateParameters();

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
    //    back to IoT Core (exercises subscribeToIotCore + publishToIotCore). Non-fatal:
    //    builds/modes without an IoT Core transport (e.g. local-only STANDALONE) skip the
    //    bridge instead of failing the whole component.
    try {
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
      this.cmdSubscribed = true;
      logger.info(`subscribed to IoT Core commands on ${this.cmdTopic}`);
    } catch (e) {
      logger.warn(`IoT Core unavailable; skipping command bridge: ${String(e)}`);
    }

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
      // Append the data point to the durable telemetry stream (partitioned by Thing). Append
      // returns once committed to the local buffer; the export engine drains to the sink.
      if (this.stream) {
        try {
          const thing = this.config.thingName;
          const payload = Buffer.from(JSON.stringify({ seq: this.seq, thing }), "utf-8");
          this.stream.append(thing, Date.now(), payload);
        } catch (e) {
          logger.warn(`failed to append to telemetry stream: ${String(e)}`);
        }
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
      if (this.cmdTopic && this.cmdSubscribed) await messaging.unsubscribeFromIotCore(this.cmdTopic);
    } catch (e) {
      logger.warn(`error while unsubscribing: ${String(e)}`);
    }
  }
}
