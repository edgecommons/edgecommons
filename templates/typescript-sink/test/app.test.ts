import { MessageBuilder, Severity } from "@edgecommons/edgecommons";
import { describe, expect, it } from "vitest";

import {
  RetryConfig,
  RetryDeps,
  Stats,
  deliverWithRetry,
  keyFor,
  parseSink,
} from "../src/app";
import { DeliverError, Delivered, Destination, Item } from "../src/dest";

// --- test doubles ------------------------------------------------------------------------------

/** A destination that fails a scripted number of times before it succeeds. */
class FlakyDestination implements Destination {
  readonly kind = "flaky";
  readonly stored = new Map<string, Buffer>();
  attempts = 0;

  constructor(
    private readonly failures: number,
    private readonly failure: DeliverError = DeliverError.transientError("endpoint unavailable"),
  ) {}

  async deliver(item: Item): Promise<Delivered> {
    this.attempts += 1;
    if (this.attempts <= this.failures) throw this.failure;
    // Idempotent: the same key overwrites, so a redelivery cannot duplicate.
    this.stored.set(item.key, item.bytes);
    return { bytesWritten: item.bytes.length };
  }

  async verify(item: Item, delivered: Delivered): Promise<void> {
    const landed = this.stored.get(item.key);
    if (!landed || landed.length !== delivered.bytesWritten) {
      throw DeliverError.transientError("what landed is not what was sent");
    }
  }
}

/** A destination whose delivery "succeeds" but lands the wrong bytes. */
class LyingDestination implements Destination {
  readonly kind = "lying";
  async deliver(): Promise<Delivered> {
    return { bytesWritten: 999 };
  }
  async verify(): Promise<void> {
    throw DeliverError.transientError("size mismatch: wrote 999 bytes, found 5");
  }
}

interface Emitted {
  severity: Severity;
  type: string;
  context?: Record<string, unknown>;
}

function recorder(): { events: { emit: (...a: never[]) => Promise<void> }; emitted: Emitted[] } {
  const emitted: Emitted[] = [];
  const events = {
    emit: async (
      severity: Severity,
      type: string,
      _message?: string,
      context?: Record<string, unknown>,
    ): Promise<void> => {
      emitted.push({ severity, type, context });
    },
  };
  return { events: events as unknown as { emit: (...a: never[]) => Promise<void> }, emitted };
}

/** A virtual clock: sleeps advance time instead of costing it. */
function fakeDeps(rand01 = 1.0): RetryDeps & { slept: number[]; clock: () => number } {
  let clock = 0;
  const slept: number[] = [];
  return {
    sleep: async (ms: number): Promise<void> => {
      slept.push(ms);
      clock += ms;
    },
    rand01: () => rand01,
    now: () => clock,
    slept,
    clock: () => clock,
  };
}

const item = (key: string, body: string): Item => ({ key, bytes: Buffer.from(body, "utf8") });

// --- config ------------------------------------------------------------------------------------

describe("sink config", () => {
  it("parses from its instance config", () => {
    const sink = parseSink({
      id: "archive",
      subscribe: "ecv1/+/+/+/data/#",
      destination: { type: "local", path: "/var/lib/out" },
      retry: { baseDelayMs: 500, giveUpAfterMs: 60000 },
    });

    expect(sink.id).toBe("archive");
    expect(sink.retry.baseDelayMs).toBe(500);
    expect(sink.retry.maxDelayMs).toBe(900_000); // the unspecified field takes its default
  });

  it("rejects an unknown config key rather than ignoring it", () => {
    expect(() =>
      parseSink({
        id: "a",
        subscribe: "t",
        destination: { type: "local", path: "/tmp" },
        retrry: {},
      }),
    ).toThrow(/unknown key/);
  });

  it("rejects a bad destination at parse time, not on the first message", () => {
    expect(() => parseSink({ id: "a", subscribe: "t", destination: { type: "s3" } })).toThrow(
      /unknown destination/,
    );
  });
});

// --- the retry ladder --------------------------------------------------------------------------

describe("retry backoff", () => {
  it("grows exponentially and is capped", () => {
    const r = new RetryConfig(1_000, 10_000, 0);
    // With full jitter, rand01 = 1.0 yields the ceiling of the window.
    expect(r.delayMs(0, 1.0)).toBe(1_000);
    expect(r.delayMs(1, 1.0)).toBe(2_000);
    expect(r.delayMs(2, 1.0)).toBe(4_000);
    // ...and it is capped, so a long outage does not back off to next week.
    expect(r.delayMs(20, 1.0)).toBe(10_000);
  });

  it("spreads the retries with full jitter", () => {
    // The point of full jitter: two components that lost the same endpoint do NOT retry in
    // lockstep. The delay is a random point in the window, not the window's edge.
    const r = new RetryConfig(1_000, 60_000, 0);
    expect(r.delayMs(3, 0.0)).toBe(0); // the window's floor is immediate
    expect(r.delayMs(3, 0.5)).toBe(4_000); // half way into an 8s window
    expect(r.delayMs(3, 1.0)).toBe(8_000);
  });

  it("gives up on a TIME BUDGET, not an attempt count", () => {
    const r = new RetryConfig(1, 1, 5_000);
    expect(r.budgetSpent(4_000)).toBe(false);
    expect(r.budgetSpent(5_000)).toBe(true);
  });
});

describe("the stable key", () => {
  it("is deterministic — the same message always resolves to the same key", () => {
    const msg = MessageBuilder.create("T", "1.0").withPayload({}).build();
    const a = keyFor("archive", "ecv1/gw/x/main/data/temp", msg);
    const b = keyFor("archive", "ecv1/gw/x/main/data/temp", msg);

    expect(a).toBe(b);
    expect(a.startsWith("archive/temp/")).toBe(true);
  });
});

describe("deliverWithRetry", () => {
  const sink = { id: "archive", retry: new RetryConfig(1_000, 10_000, 3_600_000) };

  it("delivers, verifies, and only then confirms", async () => {
    const dest = new FlakyDestination(0);
    const stats = new Stats();
    const { events, emitted } = recorder();

    await deliverWithRetry(sink, item("k.json", "hello"), dest, stats, events, fakeDeps());

    expect(stats.delivered).toBe(1);
    expect(emitted.map((e) => e.type)).toEqual(["delivery-started", "delivery-completed"]);
    expect(dest.stored.get("k.json")?.toString()).toBe("hello");
  });

  it("retries a transient failure with jittered, growing backoff and then succeeds", async () => {
    const dest = new FlakyDestination(3);
    const stats = new Stats();
    const deps = fakeDeps(1.0);
    const { events, emitted } = recorder();

    await deliverWithRetry(sink, item("k.json", "hello"), dest, stats, events, deps);

    expect(dest.attempts).toBe(4);
    expect(stats.retried).toBe(3);
    expect(stats.delivered).toBe(1);
    expect(deps.slept).toEqual([1_000, 2_000, 4_000]); // exponential, capped at 10s
    expect(emitted.map((e) => e.type)).toEqual([
      "delivery-started",
      "delivery-failed",
      "delivery-failed",
      "delivery-failed",
      "delivery-completed",
    ]);
    // "still trying" is distinguishable from "gave up".
    expect(emitted[1].context?.willRetry).toBe(true);
  });

  it("redelivery overwrites rather than duplicating", async () => {
    const dest = new FlakyDestination(1);
    const stats = new Stats();

    await deliverWithRetry(sink, item("k.json", "v1"), dest, stats, undefined, fakeDeps());
    await deliverWithRetry(sink, item("k.json", "v2"), dest, stats, undefined, fakeDeps());

    // Two deliveries, ONE object: a sink that cannot retry without duplicating cannot retry at all.
    expect(dest.stored.size).toBe(1);
    expect(dest.stored.get("k.json")?.toString()).toBe("v2");
  });

  it("does not confirm when verify catches a mismatch — it retries until the budget is spent", async () => {
    // Releasing the source because deliver() resolved, without checking what landed, is how you
    // end up having deleted the only copy.
    const stats = new Stats();
    const { events, emitted } = recorder();
    const shortBudget = { id: "archive", retry: new RetryConfig(1_000, 1_000, 3_000) };

    await deliverWithRetry(shortBudget, item("k.json", "hello"), new LyingDestination(), stats, events, fakeDeps());

    expect(stats.delivered).toBe(0);
    expect(stats.exhausted).toBe(1);
    expect(emitted.at(-1)?.type).toBe("delivery-exhausted");
    expect(emitted.at(-1)?.severity).toBe(Severity.Critical);
  });

  it("gives up immediately on a permanent failure — retrying would only burn the budget", async () => {
    const dest = new FlakyDestination(99, DeliverError.permanent("bad credentials"));
    const stats = new Stats();
    const deps = fakeDeps();
    const { events, emitted } = recorder();

    await deliverWithRetry(sink, item("k.json", "hello"), dest, stats, events, deps);

    expect(dest.attempts).toBe(1);
    expect(deps.slept).toEqual([]);
    expect(stats.retried).toBe(0);
    expect(stats.exhausted).toBe(1);
    expect(emitted.map((e) => e.type)).toEqual(["delivery-started", "delivery-exhausted"]);
    expect(emitted.at(-1)?.severity).toBe(Severity.Critical);
  });

  it("stops at the time budget, however cheap each attempt was", async () => {
    const dest = new FlakyDestination(99);
    const stats = new Stats();
    const deps = fakeDeps(1.0);
    const { events, emitted } = recorder();
    // 5s of budget, 1s base backoff: 1s + 2s -> the third check is past the budget.
    const impatient = { id: "archive", retry: new RetryConfig(1_000, 10_000, 5_000) };

    await deliverWithRetry(impatient, item("k.json", "hello"), dest, stats, events, deps);

    expect(deps.clock()).toBeGreaterThanOrEqual(5_000);
    expect(stats.exhausted).toBe(1);
    expect(stats.delivered).toBe(0);
    // Gave up must be LOUD: a sink that fails quietly is indistinguishable from one that is idle.
    expect(emitted.at(-1)?.type).toBe("delivery-exhausted");
    expect(emitted.at(-1)?.severity).toBe(Severity.Critical);
  });
});

describe("stats", () => {
  it("reset on each interval", () => {
    const stats = new Stats();
    stats.received = 2;
    stats.exhausted = 1;

    expect(stats.takeInterval()).toEqual({
      received: 2,
      delivered: 0,
      retried: 0,
      exhausted: 1,
      dropped: 0,
    });
    expect(stats.takeInterval().received).toBe(0);
  });
});
