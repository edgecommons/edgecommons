/**
 * Deterministic unit tests for {@link RepublishListener} (DESIGN-uns §9.3/§9.4, the late-join
 * lever) via the injected delayer/clock/jitter seams — no sleeping, no real scheduler (except
 * the one dedicated "production wiring" test, which uses vitest fake timers instead of a real
 * sleep). Mirrors the Java `RepublishListenerTest`.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";

import { Config } from "../src/config/model";
import { Message, MessageBuilder } from "../src/message";
import { RepublishListener } from "../src/republish_listener";
import { RecordingMessagingService } from "./_fakes";

/** The default test identity's device is `test-thing` (single 'device' level, like the Java test). */
// D-U28: the _bcast pseudo-component is component-scoped (no instance token).
const STATE_TOPIC = "ecv1/test-thing/_bcast/cmd/republish-state";
const CFG_TOPIC = "ecv1/test-thing/_bcast/cmd/republish-cfg";

const config = (): Config => Config.fromValue("com.example.C", "test-thing", {});

/** Records scheduled (task, delay) pairs; the test runs tasks synchronously on demand. */
class RecordingDelayer {
  readonly tasks: Array<() => void> = [];
  readonly delays: number[] = [];

  schedule = (task: () => void, delayMillis: number): void => {
    this.tasks.push(task);
    this.delays.push(delayMillis);
  };

  /** Runs and clears every scheduled task (the "jitter delay elapsed" step). */
  runAll(): void {
    const toRun = [...this.tasks];
    this.tasks.length = 0;
    this.delays.length = 0;
    toRun.forEach((t) => t());
  }
}

function broadcast(verb: string): Message {
  return MessageBuilder.create(verb, "1.0").withPayload({}).build();
}

/** Fetches the topic's registered handler from the recording messaging service (subscribe() call). */
function handlerFor(svc: RecordingMessagingService, topic: string): (topic: string, message: Message) => void {
  const h = svc.subscriptions.get(topic);
  if (!h) throw new Error(`no handler registered for '${topic}'`);
  return h as (topic: string, message: Message) => void;
}

describe("RepublishListener", () => {
  let messaging: RecordingMessagingService;
  let delayer: RecordingDelayer;
  let clock: number;
  let jitterWindowSeen: number;
  let nextJitter: number;
  let stateRepublishes: number;
  let cfgRepublishes: number;
  let listener: RepublishListener;

  beforeEach(() => {
    messaging = new RecordingMessagingService();
    delayer = new RecordingDelayer();
    clock = 0;
    jitterWindowSeen = -1;
    nextJitter = 0;
    stateRepublishes = 0;
    cfgRepublishes = 0;
    listener = new RepublishListener(
      config,
      messaging,
      () => {
        stateRepublishes++;
      },
      () => {
        cfgRepublishes++;
      },
      delayer.schedule,
      () => clock,
      (window) => {
        jitterWindowSeen = window;
        return nextJitter;
      },
    );
  });

  it("start() subscribes both own-device _bcast topics", async () => {
    await listener.start();
    expect(new Set(messaging.subscriptions.keys())).toEqual(new Set([STATE_TOPIC, CFG_TOPIC]));
  });

  it("republish-state re-emits the state keepalive (only)", async () => {
    await listener.start();
    handlerFor(messaging, STATE_TOPIC)(STATE_TOPIC, broadcast("republish-state"));
    expect(stateRepublishes).toBe(0); // the re-announce must wait for the jitter delay
    delayer.runAll();
    expect(stateRepublishes).toBe(1);
    expect(cfgRepublishes).toBe(0);
  });

  it("republish-cfg re-runs the effective-config publisher (only)", async () => {
    await listener.start();
    handlerFor(messaging, CFG_TOPIC)(CFG_TOPIC, broadcast("republish-cfg"));
    delayer.runAll();
    expect(cfgRepublishes).toBe(1);
    expect(stateRepublishes).toBe(0);
  });

  it("the jitter window is applied to the scheduled delay", async () => {
    nextJitter = 1234;
    await listener.start();
    handlerFor(messaging, STATE_TOPIC)(STATE_TOPIC, broadcast("republish-state"));
    expect(jitterWindowSeen).toBe(RepublishListener.JITTER_WINDOW_MS);
    expect(delayer.delays).toEqual([1234]);
  });

  it("broadcasts coalesce while a re-announce is pending", async () => {
    await listener.start();
    const h = handlerFor(messaging, STATE_TOPIC);
    h(STATE_TOPIC, broadcast("republish-state"));
    h(STATE_TOPIC, broadcast("republish-state"));
    h(STATE_TOPIC, broadcast("republish-state"));
    expect(delayer.tasks.length).toBe(1);
    delayer.runAll();
    expect(stateRepublishes).toBe(1);
  });

  it("broadcasts coalesce within the cooldown and accept again after it", async () => {
    await listener.start();
    const h = handlerFor(messaging, STATE_TOPIC);
    h(STATE_TOPIC, broadcast("republish-state"));
    delayer.runAll(); // fired; cooldown runs from the ACCEPTED trigger at t=0

    clock = RepublishListener.COOLDOWN_MS - 1;
    h(STATE_TOPIC, broadcast("republish-state"));
    expect(delayer.tasks.length).toBe(0); // inside the cooldown -> coalesced
    expect(stateRepublishes).toBe(1);

    clock = RepublishListener.COOLDOWN_MS;
    h(STATE_TOPIC, broadcast("republish-state"));
    expect(delayer.tasks.length).toBe(1); // the cooldown boundary accepts again
    delayer.runAll();
    expect(stateRepublishes).toBe(2);
  });

  it("the verbs rate-limit independently", async () => {
    await listener.start();
    handlerFor(messaging, STATE_TOPIC)(STATE_TOPIC, broadcast("republish-state"));
    // With a state re-announce pending, a cfg broadcast must still be accepted.
    handlerFor(messaging, CFG_TOPIC)(CFG_TOPIC, broadcast("republish-cfg"));
    expect(delayer.tasks.length).toBe(2);
    delayer.runAll();
    expect(stateRepublishes).toBe(1);
    expect(cfgRepublishes).toBe(1);
  });

  it("foreign and malformed payloads are ignored (never throw)", async () => {
    await listener.start();
    const h = handlerFor(messaging, STATE_TOPIC);
    // Wrong verb name in the header (foreign command on the topic).
    h(STATE_TOPIC, broadcast("something-else"));
    // A raw (headerless) envelope - e.g. junk JSON published on the broadcast topic.
    h(STATE_TOPIC, Message.fromObject({}));
    // A null message must not crash the callback either.
    expect(() => h(STATE_TOPIC, null as unknown as Message)).not.toThrow();
    expect(delayer.tasks.length).toBe(0);
    expect(stateRepublishes).toBe(0);
    expect(cfgRepublishes).toBe(0);
  });

  it("a synchronously-throwing re-announce is swallowed and does not wedge the verb", async () => {
    const failing = new RepublishListener(
      config,
      messaging,
      () => {
        throw new Error("boom");
      },
      () => {
        cfgRepublishes++;
      },
      delayer.schedule,
      () => clock,
      () => 0,
    );
    await failing.start();
    handlerFor(messaging, STATE_TOPIC)(STATE_TOPIC, broadcast("republish-state"));
    expect(() => delayer.runAll()).not.toThrow();
    // After the cooldown the verb accepts again (pending was cleared despite the failure).
    clock = RepublishListener.COOLDOWN_MS;
    handlerFor(messaging, STATE_TOPIC)(STATE_TOPIC, broadcast("republish-state"));
    expect(delayer.tasks.length).toBe(1);
    await failing.close();
  });

  it("an async (rejecting-promise) re-announce is swallowed too", async () => {
    const failing = new RepublishListener(
      config,
      messaging,
      () => Promise.reject(new Error("async boom")),
      () => {
        cfgRepublishes++;
      },
      delayer.schedule,
      () => clock,
      () => 0,
    );
    await failing.start();
    handlerFor(messaging, STATE_TOPIC)(STATE_TOPIC, broadcast("republish-state"));
    expect(() => delayer.runAll()).not.toThrow();
    // Let the rejected promise's .catch() handler run before the test ends.
    await new Promise((r) => setImmediate(r));
    await failing.close();
  });

  it("close() unsubscribes both topics and drops pending re-announces", async () => {
    await listener.start();
    handlerFor(messaging, STATE_TOPIC)(STATE_TOPIC, broadcast("republish-state"));
    const pendingTask = delayer.tasks[0];
    await listener.close();
    expect(new Set(messaging.unsubscribed)).toEqual(new Set([STATE_TOPIC, CFG_TOPIC]));
    // A pending re-announce must not fire after close() (run the captured task directly, since
    // close() unsubscribing does not itself cancel an already-scheduled timer).
    pendingTask();
    expect(stateRepublishes).toBe(0);
  });

  it("close() is idempotent and start() after close() is a no-op", async () => {
    await listener.start();
    await listener.close();
    await expect(listener.close()).resolves.toBeUndefined();
    await listener.start(); // closed -> must not resubscribe
    expect(messaging.subscriptions.size).toBe(0);
  });

  it("start() is idempotent", async () => {
    await listener.start();
    await listener.start();
    expect(new Set(messaging.subscriptions.keys())).toEqual(new Set([STATE_TOPIC, CFG_TOPIC]));
    handlerFor(messaging, STATE_TOPIC)(STATE_TOPIC, broadcast("republish-state"));
    expect(delayer.tasks.length).toBe(1); // a double start must not double-schedule
  });

  it("a subscribe failure disables the listener (best-effort start, never throws)", async () => {
    messaging.subscribe = async () => {
      throw new Error("broker unavailable");
    };
    await expect(listener.start()).resolves.toBeUndefined();
    expect(messaging.subscriptions.size).toBe(0);
    // close() after a failed start is a safe no-op (nothing was subscribed).
    await expect(listener.close()).resolves.toBeUndefined();
  });

  it("an unsubscribe failure during close() is swallowed", async () => {
    await listener.start();
    messaging.unsubscribe = async () => {
      throw new Error("already disconnected");
    };
    await expect(listener.close()).resolves.toBeUndefined();
  });

  it("a broadcast delivered mid-teardown (after close(), stale handler reference) is ignored", async () => {
    await listener.start();
    const h = handlerFor(messaging, STATE_TOPIC); // captured before close() removes the subscription
    await listener.close();
    expect(() => h(STATE_TOPIC, broadcast("republish-state"))).not.toThrow();
    expect(delayer.tasks).toHaveLength(0);
    expect(stateRepublishes).toBe(0);
  });

  it("an exception while evaluating the accept/coalesce decision is caught and logged (never crashes)", async () => {
    const brittle = new RepublishListener(
      config,
      messaging,
      () => {
        stateRepublishes++;
      },
      () => {
        cfgRepublishes++;
      },
      delayer.schedule,
      () => {
        throw new Error("clock broke");
      },
      () => 0,
    );
    await brittle.start();
    expect(() => handlerFor(messaging, STATE_TOPIC)(STATE_TOPIC, broadcast("republish-state"))).not.toThrow();
    expect(delayer.tasks).toHaveLength(0);
    expect(stateRepublishes).toBe(0);
    await brittle.close();
  });

  it("production wiring: constructs with real defaults and schedules within the jitter window", async () => {
    vi.useFakeTimers();
    try {
      const production = new RepublishListener(
        config,
        messaging,
        () => {
          stateRepublishes++;
        },
        () => {
          cfgRepublishes++;
        },
      );
      await production.start();
      expect(new Set(messaging.subscriptions.keys())).toEqual(new Set([STATE_TOPIC, CFG_TOPIC]));
      handlerFor(messaging, CFG_TOPIC)(CFG_TOPIC, broadcast("republish-cfg"));
      expect(cfgRepublishes).toBe(0);
      await vi.advanceTimersByTimeAsync(RepublishListener.JITTER_WINDOW_MS);
      expect(cfgRepublishes).toBe(1);
      await production.close();
      expect(new Set(messaging.unsubscribed)).toEqual(new Set([STATE_TOPIC, CFG_TOPIC]));
    } finally {
      vi.useRealTimers();
    }
  });
});

afterEach(() => {
  vi.restoreAllMocks();
});
