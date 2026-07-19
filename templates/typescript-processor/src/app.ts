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
import { Config, InstanceConnectivity, Message, MessageBuilder } from "@edgecommons/edgecommons";

import { ProcMsg, buildStage } from "./proc";

/** The metric this component emits each interval. */
export const METRIC_NAME = "processorThroughput";

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

// --- instance connectivity ---------------------------------------------------------------------

/**
 * The per-instance connectivity this component reports — **none**.
 *
 * A processor's routes are not connections: it consumes off the bus and publishes back onto it, and
 * the bus is the library's business, not an instance of ours. A component with no instances reports
 * none, and that is the honest answer rather than a gap — the `state` keepalive carries no
 * `instances[]` section, and the built-in `status` verb answers exactly as `ping` does
 * (`{"status":"RUNNING","uptimeSecs":n}`).
 *
 * If your processor *does* own a connection (an enrichment database, a model server it calls per
 * message), return one entry per connection instead — each a **cached** status read, never live IO:
 * the provider is sampled on the keepalive interval, and on the command path too.
 *
 * ```ts
 * return [
 *   InstanceConnectivity.of("enrichment-db", pool.isUp(), "postgres://…")
 *     .withState("BACKOFF")                          // OUR vocabulary
 *     .withAttributes({ lastError: "timeout" }),     // domain data
 * ];
 * ```
 *
 * `connected` is the one **normalized** field and is always present, so any console renders a health
 * dot for any component without knowing that component's vocabulary. `state` is our *own* token for
 * what a boolean cannot say ("reconnecting" vs "administratively disabled"), and `attributes` is an
 * open bag: domain data goes there, where it can never destabilize the fields every consumer reads.
 */
export function instanceConnectivity(): InstanceConnectivity[] {
  return [];
}

// --- the publish decision --------------------------------------------------------------------

/**
 * Restamp a pipeline output as **ours** before it goes back on the bus.
 *
 * What we publish is our identity, not the producer's — without the restamp the fleet cannot tell
 * who emitted a message, and the self-echo guard downstream ({@link isSelfEcho}) cannot work either.
 * The body and header name/version are carried through unchanged; only identity is (re)stamped, via
 * `withConfig`. The actual publish (local vs northbound, error counting, the failure event) is IO
 * and lives in the runtime seam (`src/runtime.ts`).
 */
export function restamp(config: Config, m: ProcMsg): Message {
  return MessageBuilder.create(m.msg.header.name, m.msg.header.version)
    .withPayload(m.msg.body)
    .withConfig(config)
    .build();
}
