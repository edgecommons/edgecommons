/**
 * # <<COMPONENTNAME>> — a processing component
 *
 * A **processor** subscribes to messages, transforms them, and forwards the result. This scaffold
 * wires that shape end to end; the transformation itself lives in `src/proc.ts`, which is where
 * your code goes.
 *
 * ```text
 *   subscribe(filter) ──► bounded queue ──► one loop per route ──► publish
 *                                              (Pipeline)          local | northbound
 * ```
 *
 * Each entry of `component.instances[]` is **one route**: topic filters, a pipeline of stages, and
 * a target. Routes are independent — one loop each — so a slow route cannot stall another, and the
 * per-key state inside a stage needs no coordination.
 *
 * ## Why a processor uses `messaging()` and not `data()`
 *
 * Worth reading twice, because it is the mistake this archetype invites. The `data()` facade is for
 * a component that *produces* readings: it mints its own topic from a signal id and imposes the
 * `SouthboundSignalUpdate` body. A processor is **payload-agnostic** — it republishes what it was
 * handed, on a topic its route names. Routing that through `data()` would rewrite both the topic
 * and the body, which is exactly what a republisher must not do. So: raw `gg.messaging()`, and
 * topics from config.
 *
 * ## Two guards that are not optional
 *
 * * **Self-echo.** A processor that publishes onto a class it also subscribes to will consume its
 *   own output, reprocess it, republish it, and saturate the device. {@link isSelfEcho} drops
 *   anything carrying our own identity.
 * * **Identity restamp.** What we publish is *ours*. Without the restamp the fleet cannot tell who
 *   emitted a message — and the self-echo guard downstream cannot work either.
 */
import {
  Config,
  ConfigurationChangeListener,
  EdgeCommons,
  EventsFacade,
  IMessagingService,
  Message,
  MessageBuilder,
  MetricBuilder,
  MetricService,
  Qos,
  Severity,
  logger,
  resolve,
} from "@edgecommons/edgecommons";

import { Pipeline, ProcMsg, buildStage } from "./proc";

/** The metric this component emits each interval. */
export const METRIC_NAME = "processorThroughput";

const METRIC_INTERVAL_MS = 60_000;
const DEFAULT_MAX_QUEUE = 256;
const DEFAULT_TICK_MS = 10_000;

// --- config ----------------------------------------------------------------------------------

/** Where a route's output goes. */
export type Target = "local" | "northbound";

/** One route == one entry of `component.instances[]`. */
export interface RouteConfig {
  readonly id: string;
  /** Topic filters to subscribe to. Wildcards are fine: `ecv1/+/+/+/data/#`. */
  readonly subscribe: readonly string[];
  /** The topic the result is published on. */
  readonly publishTopic: string;
  readonly target: Target;
  /** The stages, in order. An empty pipeline is a pass-through republisher. */
  readonly pipeline: readonly unknown[];
  /**
   * How many messages may be queued for this route before new ones are dropped.
   *
   * Bounded on purpose. An unbounded queue does not remove backpressure — it relocates the failure
   * to the heap, and by the time you notice you have lost the ability to report it.
   */
  readonly maxQueue: number;
  /** How often stateful stages are ticked, in milliseconds. */
  readonly tickMs: number;
}

const ROUTE_KEYS = new Set(["id", "subscribe", "publishTopic", "target", "pipeline", "maxQueue", "tickMs"]);

/**
 * Parse one entry of `component.instances[]`. Unknown keys are rejected rather than ignored: a
 * typo'd route key is a mistake, not a no-op.
 *
 * @throws Error when the entry is malformed
 */
export function parseRoute(raw: unknown): RouteConfig {
  if (typeof raw !== "object" || raw === null) throw new Error("a route must be an object");
  const o = raw as Record<string, unknown>;

  for (const key of Object.keys(o)) {
    if (!ROUTE_KEYS.has(key)) throw new Error(`unknown key '${key}'`);
  }
  if (typeof o.id !== "string" || o.id === "") throw new Error("`id` is required");
  if (typeof o.publishTopic !== "string" || o.publishTopic === "") {
    throw new Error("`publishTopic` is required");
  }

  const subscribe = o.subscribe ?? [];
  if (!Array.isArray(subscribe) || subscribe.some((f) => typeof f !== "string")) {
    throw new Error("`subscribe` must be an array of topic filters");
  }

  const target = o.target ?? "local";
  if (target !== "local" && target !== "northbound") {
    throw new Error("`target` must be `local` or `northbound`");
  }

  const pipeline = o.pipeline ?? [];
  if (!Array.isArray(pipeline)) throw new Error("`pipeline` must be an array of stages");
  pipeline.forEach(buildStage); // fail at parse time, not on the first message

  const maxQueue = o.maxQueue ?? DEFAULT_MAX_QUEUE;
  if (typeof maxQueue !== "number" || maxQueue < 1) throw new Error("`maxQueue` must be >= 1");

  const tickMs = o.tickMs ?? DEFAULT_TICK_MS;
  if (typeof tickMs !== "number" || tickMs < 1) throw new Error("`tickMs` must be >= 1");

  return {
    id: o.id,
    subscribe: subscribe as string[],
    publishTopic: o.publishTopic,
    target,
    pipeline: pipeline as unknown[],
    maxQueue,
    tickMs,
  };
}

// --- the guards ------------------------------------------------------------------------------

/**
 * Would consuming this message mean consuming our own output?
 *
 * A processor publishing onto a class it also subscribes to consumes its own output forever. The
 * guard is identity-based, not topic-based: a topic filter can be widened in config by someone who
 * has never read this file, and the loop it opens is silent until the device falls over.
 */
export function isSelfEcho(msg: Message, myPath: string, myComponent: string): boolean {
  const identity = msg.identity;
  return identity !== undefined && identity.path === myPath && identity.component === myComponent;
}

/**
 * A bounded queue that **drops and counts** when it is full.
 *
 * `push` returns `false` rather than growing: a processor that silently discards messages is worse
 * than one that crashes, so the drop is counted and reported as a metric.
 */
export class BoundedQueue<T> {
  private readonly items: T[] = [];
  private waiter?: () => void;
  private closed = false;

  constructor(readonly capacity: number) {}

  /** Enqueue, or return `false` when the queue is full (the caller counts the drop). */
  push(item: T): boolean {
    if (this.closed || this.items.length >= this.capacity) return false;
    this.items.push(item);
    this.waiter?.();
    return true;
  }

  /** Take the next item, waiting at most `timeoutMs`. Resolves `undefined` on timeout or close. */
  async receive(timeoutMs: number): Promise<T | undefined> {
    const first = this.items.shift();
    if (first !== undefined) return first;
    if (this.closed || timeoutMs <= 0) return undefined;

    await new Promise<void>((resolve) => {
      const timer = setTimeout(() => {
        this.waiter = undefined;
        resolve();
      }, timeoutMs);
      this.waiter = () => {
        clearTimeout(timer);
        this.waiter = undefined;
        resolve();
      };
    });
    return this.items.shift();
  }

  close(): void {
    this.closed = true;
    this.waiter?.();
  }

  get length(): number {
    return this.items.length;
  }
}

/** Counters, reported as a metric each interval. */
export class Stats {
  received = 0;
  published = 0;
  /**
   * Dropped because a route's queue was full. **Never let this be invisible** — a processor that
   * silently discards messages is worse than one that crashes.
   */
  dropped = 0;
  errors = 0;

  takeInterval(): Record<string, number> {
    const values = {
      received: this.received,
      published: this.published,
      dropped: this.dropped,
      errors: this.errors,
    };
    this.received = 0;
    this.published = 0;
    this.dropped = 0;
    this.errors = 0;
    return values;
  }
}

// --- the app ---------------------------------------------------------------------------------

export class App {
  private readonly config: Config;
  private readonly metrics: MetricService;
  private readonly messaging?: IMessagingService;
  private readonly events?: EventsFacade;
  private readonly routes: RouteConfig[] = [];
  private readonly stats = new Stats();
  private readonly queues = new Map<string, BoundedQueue<ProcMsg>>();
  private readonly loops: Promise<void>[] = [];
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

    // A processor with no transport is a processor with nothing to do — unlike the base scaffold,
    // this is fatal rather than a degrade-to-heartbeat.
    try {
      this.messaging = gg.messaging();
    } catch {
      throw new Error("a processor needs a messaging transport, and none was wired");
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
        .addMeasure("published", "Count", 60)
        .addMeasure("dropped", "Count", 60)
        .addMeasure("errors", "Count", 60)
        .build(),
    );

    // One route per instance. A malformed route is skipped with a warning rather than killing the
    // component — but if EVERY route is malformed there is nothing to run, and failing loudly beats
    // idling silently.
    for (const id of this.config.instanceIds()) {
      try {
        const route = parseRoute(this.config.instance(id));
        // `publishTopic` goes through the library's config-template resolver, so a deployed route
        // can name `{ThingName}` / `{ComponentName}` / a hierarchy level / a tag and still address
        // the device it actually landed on. The literal template is preserved; only the substituted
        // values are sanitized.
        this.routes.push({ ...route, publishTopic: resolve(this.config, route.publishTopic) });
      } catch (e) {
        logger.warn(`skipping malformed route '${id}': ${String(e)}`);
      }
    }
    if (this.routes.length === 0) {
      throw new Error("no valid routes in component.instances[]");
    }
  }

  async run(): Promise<void> {
    const messaging = this.messaging;
    if (!messaging) throw new Error("no messaging transport");

    // Our own identity, captured once: the self-echo guard compares against it per message.
    const myPath = this.config.componentIdentity.path;
    const myComponent = this.config.componentIdentity.component;

    for (const route of this.routes) {
      const queue = new BoundedQueue<ProcMsg>(route.maxQueue);
      this.queues.set(route.id, queue);

      for (const filter of route.subscribe) {
        await messaging.subscribe(
          filter,
          (topic: string, msg: Message) => {
            if (isSelfEcho(msg, myPath, myComponent)) {
              return; // our own output; consuming it would loop forever
            }
            this.stats.received += 1;
            // A full queue must DROP and be COUNTED, never block the transport's dispatch.
            if (!queue.push({ topic, msg })) {
              this.stats.dropped += 1;
            }
          },
          route.maxQueue,
          1,
        );
        logger.info(`route=${route.id} subscribed filter=${filter}`);
      }

      this.loops.push(
        this.runRoute(route, queue, messaging).catch((e: unknown) =>
          logger.error(`route '${route.id}' stopped: ${String(e)}`),
        ),
      );
    }

    while (!this.stopped) {
      await sleep(METRIC_INTERVAL_MS);
      await this.emitMetrics();
    }
  }

  /**
   * One route's loop. Two arms, and they are the archetype: a message arrived → run the pipeline;
   * the tick fired → let stateful stages emit. A final tick on the way out emits a half-full window
   * rather than silently losing it.
   */
  private async runRoute(
    route: RouteConfig,
    queue: BoundedQueue<ProcMsg>,
    messaging: IMessagingService,
  ): Promise<void> {
    const pipeline = new Pipeline(route.pipeline.map(buildStage));

    while (!this.stopped) {
      const m = await queue.receive(route.tickMs);
      const out = m ? pipeline.run([m]) : pipeline.run([], Date.now());
      await this.dispatch(route, out, messaging);
    }

    // A final tick on the way out, so a half-full window is emitted rather than silently lost.
    await this.dispatch(route, pipeline.run([], Date.now()), messaging);
    logger.info(`route=${route.id} stopped`);
  }

  private async dispatch(route: RouteConfig, out: readonly ProcMsg[], messaging: IMessagingService): Promise<void> {
    for (const m of out) {
      // Restamp identity: what we publish is OURS, not the producer's.
      const msg = MessageBuilder.create(m.msg.header.name, m.msg.header.version)
        .withPayload(m.msg.body)
        .withConfig(this.config)
        .build();

      try {
        if (route.target === "northbound") {
          await messaging.publishNorthbound(route.publishTopic, msg, Qos.AtLeastOnce);
        } else {
          await messaging.publish(route.publishTopic, msg);
        }
        this.stats.published += 1;
      } catch (e) {
        this.stats.errors += 1;
        logger.warn(`route=${route.id} publish failed: ${String(e)}`);
        await this.events
          ?.emit(Severity.Warning, "publish-failed", `route ${route.id} could not publish`, {
            route: route.id,
            topic: route.publishTopic,
          })
          .catch(() => undefined);
      }
    }
  }

  private async emitMetrics(): Promise<void> {
    await this.metrics
      .emitMetric(METRIC_NAME, this.stats.takeInterval())
      .catch((e: unknown) => logger.warn(`metric emit failed: ${String(e)}`));
  }

  /** Stop the route loops and clean up before the runtime is closed. */
  async stop(): Promise<void> {
    this.stopped = true;
    for (const queue of this.queues.values()) queue.close();
    await Promise.allSettled(this.loops);
    await this.emitMetrics();
    await this.metrics.flushMetrics().catch(() => undefined);
  }
}

const sleep = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));
