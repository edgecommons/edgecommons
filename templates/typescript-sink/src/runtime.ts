/**
 * # <<COMPONENTNAME>> — the runtime seam
 *
 * This is the **thin live-runtime seam**: it wires the `edgecommons` service handles together,
 * builds each destination, subscribes each sink, and hands every message to the delivery ladder. It
 * needs a live runtime (a built {@link EdgeCommons}, a messaging transport, a clock) to do anything,
 * so it is validated by the HOST / GREENGRASS / KUBERNETES deploy paths (the AGENTS.md validation
 * matrix), not by a unit test — and it is excluded from the coverage denominator in
 * `vitest.config.ts`. Everything a test can exercise without a live runtime — sink parsing, the
 * retry backoff, the stable key, the delivery ladder ({@link deliverWithRetry}), per-destination
 * connectivity, and the destination backends (`src/dest.ts`) — is covered by unit tests. Keep this
 * file free of logic a test could exercise, so the exclusion stays honest.
 */
import {
  Config,
  ConfigurationChangeListener,
  EdgeCommons,
  EventsFacade,
  IMessagingService,
  Message,
  MetricBuilder,
  MetricService,
  logger,
} from "@edgecommons/edgecommons";

import {
  DestHealth,
  METRIC_NAME,
  SinkConfig,
  Stats,
  connectivityOf,
  deliverWithRetry,
  keyFor,
  parseSink,
} from "./app";
import { Destination, Item, buildDestination } from "./dest";

const METRIC_INTERVAL_MS = 60_000;

const sleep = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));

export class App {
  private readonly config: Config;
  private readonly metrics: MetricService;
  private readonly messaging?: IMessagingService;
  private readonly events?: EventsFacade;
  private readonly sinks: SinkConfig[] = [];
  private readonly stats = new Stats();
  /** Each destination, built once at startup, and the condition its delivery ladder reports. */
  private readonly destinations = new Map<string, Destination>();
  private readonly health = new Map<string, DestHealth>();
  private readonly inFlight: Promise<void>[] = [];
  private stopped = false;

  constructor(gg: EdgeCommons) {
    this.config = gg.config();
    this.metrics = gg.metrics();

    const listener: ConfigurationChangeListener = {
      onConfigurationChange: (config: Config): boolean => {
        logger.info(`configuration changed (thing=${config.thingName})`);
        return true;
      },
    };
    gg.addConfigChangeListener(listener);

    try {
      this.messaging = gg.messaging();
    } catch {
      throw new Error("a sink needs a messaging transport, and none was wired");
    }
    try {
      this.events = gg.events();
    } catch {
      this.events = undefined;
    }

    this.metrics.defineMetric(
      MetricBuilder.create(METRIC_NAME)
        .withConfig(this.config)
        .addMeasure("received", "Count", 60)
        .addMeasure("delivered", "Count", 60)
        .addMeasure("retried", "Count", 60)
        .addMeasure("exhausted", "Count", 60)
        .addMeasure("dropped", "Count", 60)
        .build(),
    );

    for (const id of this.config.instanceIds()) {
      try {
        this.sinks.push(parseSink(this.config.instance(id)));
      } catch (e) {
        logger.warn(`skipping malformed sink '${id}': ${String(e)}`);
      }
    }
    if (this.sinks.length === 0) {
      throw new Error("no valid sinks in component.instances[]");
    }

    // A sink's destinations ARE its instances, and they exist from the moment they are CONFIGURED —
    // so every one of them is reported from the very first keepalive, before a single message has
    // been delivered. The fleet sees a bucket stop accepting data without reading one log line.
    //
    // ONE provider, TWO surfaces: the library pushes this sample into the `state` keepalive's
    // `instances[]` every tick, and returns the very same sample from the built-in `status` verb
    // when a console asks. Whoever watches and whoever asks cannot get different answers. Keep it
    // cheap — it is sampled on the keepalive interval, and it reads only cached state.
    for (const sink of this.sinks) {
      this.destinations.set(sink.id, buildDestination(sink.destination));
      this.health.set(sink.id, new DestHealth());
    }
    gg.setInstanceConnectivityProvider(() =>
      this.sinks.map((s) =>
        connectivityOf(s, this.destinations.get(s.id) as Destination, this.health.get(s.id) as DestHealth),
      ),
    );
  }

  async run(): Promise<void> {
    const messaging = this.messaging;
    if (!messaging) throw new Error("no messaging transport");

    for (const sink of this.sinks) {
      const destination = this.destinations.get(sink.id) as Destination;
      const health = this.health.get(sink.id) as DestHealth;

      // Deliveries run one at a time per sink (maxConcurrency 1): a bounded, ordered pipeline whose
      // backpressure is the transport's own queue bound rather than an unbounded heap of promises.
      await messaging.subscribe(
        sink.subscribe,
        async (topic: string, msg: Message) => {
          this.stats.received += 1;
          if (this.stopped) {
            this.stats.dropped += 1;
            return;
          }
          const item: Item = {
            // A stable, deterministic key: the same message always lands in the same place, so a
            // redelivery overwrites.
            key: keyFor(sink.id, topic, msg),
            bytes: Buffer.from(JSON.stringify(msg.body ?? null), "utf8"),
          };
          const delivery = deliverWithRetry(sink, item, destination, this.stats, health, this.events);
          this.inFlight.push(delivery);
          await delivery;
        },
        sink.maxQueue,
        1,
      );
      logger.info(`sink=${sink.id} subscribed filter=${sink.subscribe}`);
    }

    while (!this.stopped) {
      await sleep(METRIC_INTERVAL_MS);
      await this.emitMetrics();
    }
  }

  private async emitMetrics(): Promise<void> {
    await this.metrics
      .emitMetric(METRIC_NAME, this.stats.takeInterval())
      .catch((e: unknown) => logger.warn(`metric emit failed: ${String(e)}`));
  }

  /** Stop accepting work, let the in-flight deliveries finish, and report one last time. */
  async stop(): Promise<void> {
    this.stopped = true;
    await Promise.allSettled(this.inFlight);
    await this.emitMetrics();
    await this.metrics.flushMetrics().catch(() => undefined);
  }
}
