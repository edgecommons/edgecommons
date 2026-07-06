import { describe, it, expect, vi } from "vitest";

import { MessageBuilder } from "../src/message";
import { DefaultMessagingService } from "../src/messaging/service";
import { Destination, Qos } from "../src/messaging/types";
import { FakeMessagingProvider, tick } from "./_fakes";

describe("DefaultMessagingService extra coverage", () => {
  it("unsubscribe stops delivery", async () => {
    const provider = new FakeMessagingProvider();
    const svc = new DefaultMessagingService(provider);
    const got: number[] = [];
    await svc.subscribe("evt", (_t, m) => {
      got.push((m.getBody() as { n: number }).n);
    });
    await svc.publish("evt", MessageBuilder.create("e", "1").withPayload({ n: 1 }).build());
    await tick();
    await svc.unsubscribe("evt");
    await svc.publish("evt", MessageBuilder.create("e", "1").withPayload({ n: 2 }).build());
    await tick();
    expect(got).toEqual([1]);
  });

  it("subscribe replace closes the old subscription", async () => {
    const provider = new FakeMessagingProvider();
    const svc = new DefaultMessagingService(provider);
    const a: string[] = [];
    const b: string[] = [];
    await svc.subscribe("dup", () => a.push("a"));
    await svc.subscribe("dup", () => b.push("b"));
    // Only one underlying sub remains on the provider after the replace.
    expect(provider.subs).toHaveLength(1);
    await svc.publish("dup", MessageBuilder.create("e", "1").build());
    await tick();
    expect(a).toEqual([]);
    expect(b).toEqual(["b"]);
  });

  it("request times out and rejects after timeoutMs", async () => {
    const provider = new FakeMessagingProvider();
    const svc = new DefaultMessagingService(provider);
    // No responder subscribed: the request will never get a reply.
    const req = MessageBuilder.create("ask", "1").withPayload({}).build();
    await expect(svc.request("no/responder", req, 30)).rejects.toThrow(/timed out/);
  });

  it("cancelRequest rejects the future and unsubscribes the reply topic", async () => {
    const provider = new FakeMessagingProvider();
    const svc = new DefaultMessagingService(provider);
    const req = MessageBuilder.create("ask", "1").build();
    const fut = svc.request("rpc", req, 0);
    await tick(); // let the reply subscription register
    const subsBefore = provider.subs.length;
    expect(subsBefore).toBeGreaterThan(0);
    svc.cancelRequest(fut);
    await expect(fut).rejects.toThrow(/canceled/);
    await tick();
    expect(provider.subs.length).toBeLessThan(subsBefore);
  });

  it("reply with no reply_to throws", async () => {
    const provider = new FakeMessagingProvider();
    const svc = new DefaultMessagingService(provider);
    const req = MessageBuilder.create("ask", "1").build(); // no reply_to
    const reply = MessageBuilder.create("ans", "1").build();
    await expect(svc.reply(req, reply)).rejects.toThrow(/no reply_to/);
  });

  it("publishNorthbound / publishNorthboundRaw route to the IoT Core destination", async () => {
    const provider = new FakeMessagingProvider();
    const svc = new DefaultMessagingService(provider);
    await svc.publishNorthbound("iot/t", MessageBuilder.create("m", "1").withPayload({ a: 1 }).build(), Qos.AtMostOnce);
    await svc.publishNorthboundRaw("iot/raw", { b: 2 });
    expect(provider.published).toHaveLength(2);
    expect(provider.published[0].dest).toBe(Destination.Northbound);
    expect(provider.published[0].qos).toBe(Qos.AtMostOnce);
    expect(provider.published[1].dest).toBe(Destination.Northbound);
  });

  it("uses configured MQTT QoS defaults for operations without an explicit QoS", async () => {
      const provider = new FakeMessagingProvider();
      const svc = new DefaultMessagingService(provider, {
        local: { publish: Qos.ExactlyOnce, subscribe: Qos.AtMostOnce },
        northbound: { publish: Qos.ExactlyOnce, subscribe: Qos.AtLeastOnce },
    });

    await svc.publish("local/qos", MessageBuilder.create("m", "1").build());
    await svc.publishRaw("local/raw/qos", { ok: true });
    const req = MessageBuilder.create("ask", "1").build();
    const fut = svc.requestNorthbound("iot/qos/request", req, 0);
    void fut.then(() => undefined, () => undefined);
    await tick();
    svc.cancelRequestNorthbound(fut);

    expect(provider.published[0].dest).toBe(Destination.Local);
    expect(provider.published[0].qos).toBe(Qos.ExactlyOnce);
    expect(provider.published[1].qos).toBe(Qos.ExactlyOnce);
    expect(provider.published[2].dest).toBe(Destination.Northbound);
    expect(provider.published[2].qos).toBe(Qos.ExactlyOnce);
  });

  it("IoT Core subscribe + request/reply round-trips", async () => {
    const provider = new FakeMessagingProvider();
    const svc = new DefaultMessagingService(provider);
    await svc.subscribeNorthbound("rpc/iot", async (_t, req) => {
      const reply = MessageBuilder.create("reply", "1").withPayload({ echoed: req.getBody() }).build();
      await svc.replyNorthbound(req, reply);
    });
    const req = MessageBuilder.create("ask", "1").withPayload({ q: 7 }).build();
    const reply = await svc.requestNorthbound("rpc/iot", req, 1000);
    expect(reply.getBody()).toEqual({ echoed: { q: 7 } });
    expect(reply.getCorrelationId()).toBe(req.getCorrelationId());
  });

  it("cancelRequestNorthbound rejects the IoT Core request", async () => {
    const provider = new FakeMessagingProvider();
    const svc = new DefaultMessagingService(provider);
    const fut = svc.requestNorthbound("noresp", MessageBuilder.create("x", "1").build(), 0);
    await tick();
    svc.cancelRequestNorthbound(fut);
    await expect(fut).rejects.toThrow(/canceled/);
  });

  it("unsubscribeNorthbound stops IoT Core delivery", async () => {
    const provider = new FakeMessagingProvider();
    const svc = new DefaultMessagingService(provider);
    const got: number[] = [];
    await svc.subscribeNorthbound("iot/evt", (_t, m) => got.push((m.getBody() as { n: number }).n));
    await svc.publishNorthbound("iot/evt", MessageBuilder.create("e", "1").withPayload({ n: 1 }).build());
    await tick();
    await svc.unsubscribeNorthbound("iot/evt");
    await svc.publishNorthbound("iot/evt", MessageBuilder.create("e", "1").withPayload({ n: 2 }).build());
    await tick();
    expect(got).toEqual([1]);
  });

  it("disconnect closes subscriptions and the provider", async () => {
    const provider = new FakeMessagingProvider();
    const svc = new DefaultMessagingService(provider);
    await svc.subscribe("a", () => undefined);
    await svc.disconnect();
    expect(provider.disconnected).toBe(true);
  });

  it("queue overflow drops messages with a warning", async () => {
    const provider = new FakeMessagingProvider();
    const svc = new DefaultMessagingService(provider);
    const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    const seen: number[] = [];
    // maxMessages=1, maxConcurrency=1, slow handler so the queue overflows.
    await svc.subscribe(
      "flood",
      async (_t, m) => {
        await tick(40);
        seen.push((m.getBody() as { n: number }).n);
      },
      1,
      1,
    );
    for (let n = 0; n < 5; n++) {
      await svc.publish("flood", MessageBuilder.create("e", "1").withPayload({ n }).build());
    }
    await tick(200);
    expect(warn).toHaveBeenCalled();
    expect(seen.length).toBeLessThan(5);
    vi.restoreAllMocks();
  });
});
