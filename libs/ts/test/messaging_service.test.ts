import { describe, it, expect } from "vitest";

import { Message, MessageBuilder } from "../src/message";
import { DefaultMessagingService } from "../src/messaging/service";
import {
  Destination,
  MessagingProvider,
  Qos,
  RawSubscription,
} from "../src/messaging/types";

/**
 * A simple in-memory MessagingProvider: a Map of topic -> subscribers, with
 * exact-topic match. Sufficient for the service-level round-trip, raw, and
 * request/reply tests (the reply topic is an exact string).
 */
class FakeProvider implements MessagingProvider {
  private readonly subs = new Map<string, Set<(t: string, p: Buffer) => void>>();
  public publishedTopics: string[] = [];

  async publishBytes(
    topic: string,
    payload: Buffer,
    _dest: Destination,
    _qos: Qos,
  ): Promise<void> {
    this.publishedTopics.push(topic);
    const handlers = this.subs.get(topic);
    if (handlers) {
      // Copy to avoid mutation-during-iteration when handlers (un)subscribe.
      for (const h of [...handlers]) {
        h(topic, payload);
      }
    }
  }

  async subscribeRaw(
    filter: string,
    _dest: Destination,
    _qos: Qos,
    onMessage: (topic: string, payload: Buffer) => void,
  ): Promise<RawSubscription> {
    let set = this.subs.get(filter);
    if (!set) {
      set = new Set();
      this.subs.set(filter, set);
    }
    set.add(onMessage);
    return {
      unsubscribe: async () => {
        set!.delete(onMessage);
      },
    };
  }

  connected(): boolean {
    return true;
  }

  async disconnect(): Promise<void> {
    this.subs.clear();
  }
}

function nextTick(): Promise<void> {
  return new Promise((r) => setImmediate(r));
}

describe("DefaultMessagingService over a fake provider", () => {
  it("publish -> subscribe round-trip delivers a decoded Message", async () => {
    const svc = new DefaultMessagingService(new FakeProvider());
    const received: Message[] = [];
    await svc.subscribe("evt/topic", (_t, m) => {
      received.push(m);
    });

    const msg = MessageBuilder.create("evt", "1.0.0").withPayload({ n: 1 }).build();
    await svc.publish("evt/topic", msg);
    await nextTick();

    expect(received).toHaveLength(1);
    expect(received[0].isRaw()).toBe(false);
    expect(received[0].header.name).toBe("evt");
    expect(received[0].getBody()).toEqual({ n: 1 });
  });

  it("publishRaw delivers a raw message", async () => {
    const svc = new DefaultMessagingService(new FakeProvider());
    const received: Message[] = [];
    await svc.subscribe("raw/topic", (_t, m) => {
      received.push(m);
    });

    await svc.publishRaw("raw/topic", { hello: "world" });
    await nextTick();

    expect(received).toHaveLength(1);
    expect(received[0].isRaw()).toBe(true);
    expect(received[0].getRaw()).toEqual({ hello: "world" });
  });

  it("request/reply correlation round-trips", async () => {
    const svc = new DefaultMessagingService(new FakeProvider());

    // Responder: echoes the request body back, copying correlation_id via reply().
    await svc.subscribe("rpc/echo", async (_t, req) => {
      const reply = MessageBuilder.create("reply", "1.0.0")
        .withPayload({ echoed: req.getBody() })
        .build();
      await svc.reply(req, reply);
    });

    const req = MessageBuilder.create("ask", "1.0.0").withPayload({ q: 5 }).build();
    const reply = await svc.request("rpc/echo", req, 1000);

    expect(reply.getBody()).toEqual({ echoed: { q: 5 } });
    // correlation id of the reply matches the request
    expect(reply.getCorrelationId()).toBe(req.getCorrelationId());
  });

  it("maxConcurrency=1 processes messages in order", async () => {
    const svc = new DefaultMessagingService(new FakeProvider());
    const order: number[] = [];

    await svc.subscribe(
      "ordered",
      async (_t, m) => {
        const n = (m.getBody() as { n: number }).n;
        // Larger delay for the first message; with concurrency 1 it must still
        // complete before the second is started.
        await new Promise((r) => setTimeout(r, n === 1 ? 20 : 1));
        order.push(n);
      },
      32,
      1,
    );

    await svc.publish("ordered", MessageBuilder.create("e", "1").withPayload({ n: 1 }).build());
    await svc.publish("ordered", MessageBuilder.create("e", "1").withPayload({ n: 2 }).build());
    await svc.publish("ordered", MessageBuilder.create("e", "1").withPayload({ n: 3 }).build());

    await new Promise((r) => setTimeout(r, 100));
    expect(order).toEqual([1, 2, 3]);
  });
});
