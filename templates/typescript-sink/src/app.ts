/**
 * # <<COMPONENTNAME>> — a sink component
 *
 * A **sink** is the last thing standing between data and its destination. It consumes work,
 * delivers it outward, and only then lets go of the source.
 *
 * ```text
 *   consume ──► deliver (idempotent, stable key) ──► verify ──► confirm ──► report
 *                        ▲                                                    │
 *                        └────────── retry with full jitter ◄─────────────────┘
 * ```
 *
 * The ordering is the archetype, and every step earns its place:
 *
 * * **Deliver idempotently, to a stable key.** A redelivery overwrites; it does not duplicate. A
 *   sink that cannot retry without duplicating cannot retry at all.
 * * **Verify before you confirm.** Trusting `deliver`'s success and releasing the source without
 *   checking what actually landed is how you end up having deleted the only copy.
 * * **Classify the failure.** Retrying a permanent error burns the budget; giving up on a transient
 *   one loses data a second attempt would have delivered. See {@link DeliverError}.
 * * **Report every transition.** A sink that fails quietly is indistinguishable from one that is
 *   idle. Started / completed / failed / exhausted all go out on the UNS event surface.
 *
 * ## Where the work comes from
 *
 * This scaffold's source is a **subscription**: it consumes messages off the bus and delivers each
 * one. That is the common case. If your source is a watched directory or a polled API, replace the
 * subscribe call in {@link App.run} — everything downstream of {@link deliverWithRetry} is
 * unchanged, which is the point of the seam.
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
  Severity,
  logger,
} from "@edgecommons/edgecommons";

import { DeliverError, Destination, Item, buildDestination } from "./dest";

export const METRIC_NAME = "sinkDeliveries";

const METRIC_INTERVAL_MS = 60_000;
const DEFAULT_MAX_QUEUE = 256;

// --- config ----------------------------------------------------------------------------------

/**
 * How hard, and for how long, to keep trying.
 *
 * Note the give-up is a **time budget**, not an attempt count. "Twenty attempts" means something
 * different at 1 s and at 15 min of backoff; "keep trying for an hour" means the same thing at every
 * cadence, and it is what an operator can actually reason about.
 */
export class RetryConfig {
  constructor(
    readonly baseDelayMs = 1_000,
    readonly maxDelayMs = 900_000, // 15 min
    readonly giveUpAfterMs = 3_600_000, // 1 hour
  ) {}

  /**
   * Full-jitter exponential backoff: a random delay in `[0, min(cap, base * 2^attempt))`.
   *
   * The jitter is not decoration. Without it, every component that lost the same endpoint retries at
   * the same instant, and the endpoint — which is probably struggling already — is hit by a
   * synchronized thundering herd on every backoff boundary.
   */
  delayMs(attempt: number, rand01: number): number {
    const exp = this.baseDelayMs * 2 ** Math.min(attempt, 20);
    const cap = Math.min(exp, this.maxDelayMs);
    return Math.floor(Math.min(Math.max(rand01, 0), 1) * cap);
  }

  budgetSpent(elapsedMs: number): boolean {
    return elapsedMs >= this.giveUpAfterMs;
  }
}

/** One sink instance == one entry of `component.instances[]`. */
export interface SinkConfig {
  readonly id: string;
  /** The topic filter whose messages this sink delivers. */
  readonly subscribe: string;
  /** Where they go. */
  readonly destination: unknown;
  readonly retry: RetryConfig;
  /** Bounded, like every queue that faces a network. */
  readonly maxQueue: number;
}

const SINK_KEYS = new Set(["id", "subscribe", "destination", "retry", "maxQueue"]);
const RETRY_KEYS = new Set(["baseDelayMs", "maxDelayMs", "giveUpAfterMs"]);

/**
 * Parse one entry of `component.instances[]`. Unknown keys are rejected rather than ignored.
 *
 * @throws Error when the entry is malformed
 */
export function parseSink(raw: unknown): SinkConfig {
  if (typeof raw !== "object" || raw === null) throw new Error("a sink must be an object");
  const o = raw as Record<string, unknown>;

  for (const key of Object.keys(o)) {
    if (!SINK_KEYS.has(key)) throw new Error(`unknown key '${key}'`);
  }
  if (typeof o.id !== "string" || o.id === "") throw new Error("`id` is required");
  if (typeof o.subscribe !== "string" || o.subscribe === "") throw new Error("`subscribe` is required");
  if (o.destination === undefined) throw new Error("`destination` is required");
  buildDestination(o.destination); // fail at parse time, not on the first message

  const defaults = new RetryConfig();
  let retry = defaults;
  if (o.retry !== undefined) {
    if (typeof o.retry !== "object" || o.retry === null) throw new Error("`retry` must be an object");
    const r = o.retry as Record<string, unknown>;
    for (const key of Object.keys(r)) {
      if (!RETRY_KEYS.has(key)) throw new Error(`unknown key 'retry.${key}'`);
    }
    retry = new RetryConfig(
      num(r.baseDelayMs, defaults.baseDelayMs, "retry.baseDelayMs"),
      num(r.maxDelayMs, defaults.maxDelayMs, "retry.maxDelayMs"),
      num(r.giveUpAfterMs, defaults.giveUpAfterMs, "retry.giveUpAfterMs"),
    );
  }

  const maxQueue = o.maxQueue ?? DEFAULT_MAX_QUEUE;
  if (typeof maxQueue !== "number" || maxQueue < 1) throw new Error("`maxQueue` must be >= 1");

  return { id: o.id, subscribe: o.subscribe, destination: o.destination, retry, maxQueue };
}

function num(value: unknown, fallback: number, what: string): number {
  if (value === undefined) return fallback;
  if (typeof value !== "number" || value < 1) throw new Error(`\`${what}\` must be a positive number`);
  return value;
}

// --- the stable key --------------------------------------------------------------------------

/**
 * A stable, deterministic key for a message.
 *
 * Deterministic is the whole point: the same message must always resolve to the same key, or a retry
 * duplicates instead of overwriting.
 */
export function keyFor(sinkId: string, topic: string, msg: Message): string {
  const leaf = topic.split("/").pop() || "message";
  return `${sinkId}/${leaf}/${msg.header.uuid}.json`;
}

// --- delivery --------------------------------------------------------------------------------

/** Counters, reported as a metric each interval. */
export class Stats {
  received = 0;
  delivered = 0;
  retried = 0;
  /** Gave up. This is the number that matters: it is data that did not arrive. */
  exhausted = 0;
  dropped = 0;

  takeInterval(): Record<string, number> {
    const values = {
      received: this.received,
      delivered: this.delivered,
      retried: this.retried,
      exhausted: this.exhausted,
      dropped: this.dropped,
    };
    this.received = 0;
    this.delivered = 0;
    this.retried = 0;
    this.exhausted = 0;
    this.dropped = 0;
    return values;
  }
}

/** The clock/sleep/jitter seam — injected so the retry ladder is testable without real waiting. */
export interface RetryDeps {
  sleep: (ms: number) => Promise<void>;
  rand01: () => number;
  now: () => number;
}

export const defaultRetryDeps: RetryDeps = {
  sleep: (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
  rand01: () => Math.random(),
  now: () => Date.now(),
};

/**
 * Deliver one item, retrying transient failures until the time budget is spent.
 *
 * The event ladder is the sink's contract with whoever is watching: **delivery-started**, then either
 * **delivery-completed**, or **delivery-failed** (with `willRetry`), and finally
 * **delivery-exhausted** (Critical) when the budget runs out. An operator must be able to tell
 * "still trying" from "gave up", and gave-up must be loud.
 */
export async function deliverWithRetry(
  sink: Pick<SinkConfig, "id" | "retry">,
  item: Item,
  destination: Destination,
  stats: Stats,
  events: Pick<EventsFacade, "emit"> | undefined,
  deps: RetryDeps = defaultRetryDeps,
): Promise<void> {
  const started = deps.now();
  let attempt = 0;

  await events
    ?.emit(Severity.Info, "delivery-started", undefined, {
      sink: sink.id,
      key: item.key,
      kind: destination.kind,
    })
    .catch(() => undefined);

  for (;;) {
    let failure: unknown;
    try {
      // deliver, then VERIFY. Only a verified delivery is a delivery.
      const delivered = await destination.deliver(item);
      await destination.verify(item, delivered);

      stats.delivered += 1;
      await events
        ?.emit(Severity.Info, "delivery-completed", undefined, {
          sink: sink.id,
          key: item.key,
          attempts: attempt + 1,
          elapsedMs: deps.now() - started,
        })
        .catch(() => undefined);
      // The source is released HERE — after verification, never before.
      return;
    } catch (e) {
      failure = e;
    }

    // Permanent: it will fail identically forever. Retrying is a waste of the budget and of the
    // log; give up now and say so.
    if (!DeliverError.isTransient(failure)) {
      stats.exhausted += 1;
      logger.error(`permanent failure sink=${sink.id} key=${item.key}: ${String(failure)}`);
      await events
        ?.emit(
          Severity.Critical,
          "delivery-exhausted",
          `${sink.id} will never deliver ${item.key}`,
          { sink: sink.id, key: item.key, reason: String(failure) },
        )
        .catch(() => undefined);
      return;
    }

    if (sink.retry.budgetSpent(deps.now() - started)) {
      stats.exhausted += 1;
      logger.error(`retry budget spent sink=${sink.id} key=${item.key} attempts=${attempt + 1}`);
      await events
        ?.emit(Severity.Critical, "delivery-exhausted", `${sink.id} gave up on ${item.key}`, {
          sink: sink.id,
          key: item.key,
          attempts: attempt + 1,
          reason: String(failure),
        })
        .catch(() => undefined);
      return;
    }

    const backoff = sink.retry.delayMs(attempt, deps.rand01());
    stats.retried += 1;
    logger.warn(
      `transient failure sink=${sink.id} key=${item.key} attempt=${attempt} ` +
        `backoffMs=${backoff}; retrying: ${String(failure)}`,
    );
    await events
      ?.emit(Severity.Warning, "delivery-failed", undefined, {
        sink: sink.id,
        key: item.key,
        attempt: attempt + 1,
        willRetry: true,
        nextAttemptInMs: backoff,
      })
      .catch(() => undefined);

    await deps.sleep(backoff);
    attempt += 1;
  }
}

// --- the app ---------------------------------------------------------------------------------

export class App {
  private readonly config: Config;
  private readonly metrics: MetricService;
  private readonly messaging?: IMessagingService;
  private readonly events?: EventsFacade;
  private readonly sinks: SinkConfig[] = [];
  private readonly stats = new Stats();
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
  }

  async run(): Promise<void> {
    const messaging = this.messaging;
    if (!messaging) throw new Error("no messaging transport");

    for (const sink of this.sinks) {
      const destination = buildDestination(sink.destination);

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
          const delivery = deliverWithRetry(sink, item, destination, this.stats, this.events);
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

const sleep = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));
