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
 *   idle. Started / completed / failed / exhausted all go out on the UNS event surface — and the
 *   same transitions move each destination's reported connectivity ({@link connectivityOf}), because
 *   a sink's destinations **are** its instances.
 *
 * ## Where the work comes from
 *
 * This scaffold's source is a **subscription**: it consumes messages off the bus and delivers each
 * one. That is the common case. If your source is a watched directory or a polled API, replace the
 * subscribe call in the runtime seam (`src/runtime.ts`) — everything downstream of
 * {@link deliverWithRetry} is unchanged, which is the point of the seam.
 *
 * This module holds the sink's **pure, unit-tested logic**: config parsing, the retry backoff, the
 * stable delivery key, the delivery ladder, and per-destination connectivity. The runtime that wires
 * the `edgecommons` handles together and drives the subscription loops lives in `src/runtime.ts`,
 * which is excluded from the coverage gate (it needs a live runtime; see `vitest.config.ts`).
 */
import { EventsFacade, InstanceConnectivity, Message, Severity, logger } from "@edgecommons/edgecommons";

import { DeliverError, Destination, Item, buildDestination } from "./dest";

export const METRIC_NAME = "sinkDeliveries";

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

// --- per-destination connectivity ----------------------------------------------------------------

/**
 * This sink's **own vocabulary** for a destination's condition — what it reports as
 * `InstanceConnectivity.state`. The delivery ladder in {@link deliverWithRetry} moves it, so what
 * the events say and what the connectivity says are the same story.
 *
 * `BACKOFF` (still trying) and `FAILED` (gave up) are the same boolean and very different pages at
 * 3 a.m. — which is exactly why the normalized flag is not enough on its own.
 */
export type DestState = "IDLE" | "ONLINE" | "BACKOFF" | "FAILED";

/** One destination's condition: written by the delivery ladder, read by the connectivity provider. */
export class DestHealth {
  /** Nothing delivered yet, so nothing has failed yet. */
  state: DestState = "IDLE";

  set(state: DestState): void {
    this.state = state;
  }

  /**
   * The normalized flag: is this destination taking data? An untried destination is not a broken
   * one, so `IDLE` reports reachable until a delivery proves otherwise.
   */
  get connected(): boolean {
    return this.state === "IDLE" || this.state === "ONLINE";
  }
}

/** A human "where the data goes", for the connectivity detail: `local:/var/lib/out`. */
export function destinationDetail(cfg: unknown): string | undefined {
  const o = typeof cfg === "object" && cfg !== null ? (cfg as Record<string, unknown>) : undefined;
  if (!o || typeof o.type !== "string") return undefined;
  return typeof o.path === "string" ? `${o.type}:${o.path}` : o.type;
}

/**
 * One destination's connectivity sample, for the provider the runtime registers (`src/runtime.ts`).
 *
 * * `connected` is the **normalized** flag — always present, so a console renders a health dot for
 *   this sink without knowing what an object store is.
 * * `state` is *this sink's* vocabulary ({@link DestState}), which is what separates a destination
 *   we are still retrying from one we have given up on.
 * * `attributes` is the **open** bag: domain data only this sink understands (here, the kind of
 *   backend), carried without destabilizing the two fields above that everyone reads.
 */
export function connectivityOf(
  sink: SinkConfig,
  destination: Destination,
  health: DestHealth,
): InstanceConnectivity {
  return InstanceConnectivity.of(sink.id, health.connected, destinationDetail(sink.destination))
    .withState(health.state)
    .withAttributes({ destination: destination.kind });
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
 *
 * Every rung of that ladder also moves `health` — the same distinction, reported as this
 * destination's {@link DestState} on the `state` keepalive and to the `status` verb.
 */
export async function deliverWithRetry(
  sink: Pick<SinkConfig, "id" | "retry">,
  item: Item,
  destination: Destination,
  stats: Stats,
  health: DestHealth,
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
      // ONLINE only once the delivery is VERIFIED — "deliver() resolved" is not yet a delivery, and
      // reporting a destination healthy on that basis is the same lie as releasing the source on it.
      health.set("ONLINE");
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
      health.set("FAILED");
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
      health.set("FAILED");
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
    health.set("BACKOFF");
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

