import { Config, MessageBuilder } from "@edgecommons/edgecommons";
import { describe, expect, it } from "vitest";

import { BoundedQueue, Stats, instanceConnectivity, isSelfEcho, parseRoute, restamp } from "../src/app";
import { ProcMsg } from "../src/proc";

const config = Config.fromValue("com.example.Processor", "gw-01", {
  hierarchy: { levels: ["site", "device"] },
  identity: { site: "factory-1" },
  component: { global: {}, instances: [{ id: "rollup" }] },
});

describe("route config", () => {
  it("parses from its instance config", () => {
    const route = parseRoute({
      id: "temps",
      subscribe: ["ecv1/+/+/+/data/#"],
      publishTopic: "ecv1/gw-01/proc/temps/app/rollup",
      target: "northbound",
      pipeline: [{ fieldEquals: { path: "signal.id", value: "temp-1" } }, { countPerTick: {} }],
      tickMs: 5000,
    });

    expect(route.id).toBe("temps");
    expect(route.target).toBe("northbound");
    expect(route.pipeline).toHaveLength(2);
    expect(route.tickMs).toBe(5_000);
    expect(route.maxQueue).toBe(256); // the queue is bounded by default
  });

  it("defaults to the common case", () => {
    const route = parseRoute({ id: "r", publishTopic: "t" });
    expect(route.target).toBe("local"); // the device-local bus is the common target
    expect(route.pipeline).toHaveLength(0); // no stages == a pass-through republisher
  });

  it("rejects an unknown config key rather than ignoring it", () => {
    expect(() => parseRoute({ id: "r", publishTopic: "t", pipelnie: [] })).toThrow(/unknown key/);
  });

  it("rejects a bad stage at parse time, not on the first message", () => {
    expect(() => parseRoute({ id: "r", publishTopic: "t", pipeline: [{ nope: {} }] })).toThrow(
      /unknown stage/,
    );
  });
});

describe("the self-echo guard", () => {
  it("drops a message carrying our own identity", () => {
    // A processor that publishes onto a class it also subscribes to would consume its own output,
    // reprocess it, republish it, and saturate the device.
    const mine = MessageBuilder.create("Rollup", "1.0").withPayload({}).withConfig(config).build();

    expect(
      isSelfEcho(mine, config.componentIdentity.path, config.componentIdentity.component),
    ).toBe(true);
  });

  it("keeps a message from another component on the same device", () => {
    const otherComponent = Config.fromValue("com.example.Adapter", "gw-01", {
      hierarchy: { levels: ["site", "device"] },
      identity: { site: "factory-1" },
      component: { global: {}, instances: [] },
    });
    const theirs = MessageBuilder.create("Data", "1.0")
      .withPayload({})
      .withConfig(otherComponent)
      .build();

    expect(
      isSelfEcho(theirs, config.componentIdentity.path, config.componentIdentity.component),
    ).toBe(false);
  });

  it("keeps a message from the same component on another device", () => {
    const otherDevice = Config.fromValue("com.example.Processor", "gw-02", {
      hierarchy: { levels: ["site", "device"] },
      identity: { site: "factory-1" },
      component: { global: {}, instances: [] },
    });
    const theirs = MessageBuilder.create("Rollup", "1.0")
      .withPayload({})
      .withConfig(otherDevice)
      .build();

    expect(
      isSelfEcho(theirs, config.componentIdentity.path, config.componentIdentity.component),
    ).toBe(false);
  });

  it("keeps a message with no identity at all", () => {
    const anonymous = MessageBuilder.create("Raw", "1.0").withPayload({}).build();
    expect(
      isSelfEcho(anonymous, config.componentIdentity.path, config.componentIdentity.component),
    ).toBe(false);
  });
});

describe("the instance-connectivity provider", () => {
  it("reports no instances, because a processor's routes are not connections", () => {
    // The provider the `state` keepalive pushes and the `status` verb pulls — one source, two
    // surfaces. Reporting nothing is the contract, not an omission: with no instances the keepalive
    // carries no `instances[]` section and `status` answers exactly as `ping` does.
    expect(instanceConnectivity()).toEqual([]);
  });
});

describe("the bounded queue", () => {
  const item = (i: number): ProcMsg => ({
    topic: "t",
    msg: MessageBuilder.create("T", "1.0").withPayload({ i }).build(),
  });

  it("drops and counts when it is full, rather than growing without bound", async () => {
    const queue = new BoundedQueue<ProcMsg>(2);
    const stats = new Stats();

    for (let i = 0; i < 5; i += 1) {
      stats.received += 1;
      if (!queue.push(item(i))) stats.dropped += 1;
    }

    expect(queue.length).toBe(2);
    expect(stats.received).toBe(5);
    // A processor that silently discards messages is worse than one that crashes: the drop is
    // counted, and the counter is reported as a metric.
    expect(stats.dropped).toBe(3);
  });

  it("resolves undefined on the tick deadline when nothing arrived", async () => {
    const queue = new BoundedQueue<ProcMsg>(2);
    expect(await queue.receive(10)).toBeUndefined();
  });

  it("hands a queued message to the route loop", async () => {
    const queue = new BoundedQueue<ProcMsg>(2);
    queue.push(item(1));
    const got = await queue.receive(1_000);
    expect((got?.msg.body as { i: number }).i).toBe(1);
  });

  it("wakes its consumer when closed, so shutdown is not a tick away", async () => {
    const queue = new BoundedQueue<ProcMsg>(2);
    const pending = queue.receive(60_000);
    queue.close();
    expect(await pending).toBeUndefined();
    expect(queue.push(item(1))).toBe(false);
  });
});

describe("stats", () => {
  it("reset on each interval, so a counter reports the interval and not all of history", () => {
    const stats = new Stats();
    stats.received = 3;
    stats.published = 2;
    stats.dropped = 1;

    expect(stats.takeInterval()).toEqual({ received: 3, published: 2, dropped: 1, errors: 0 });
    expect(stats.takeInterval()).toEqual({ received: 0, published: 0, dropped: 0, errors: 0 });
  });
});

describe("the identity restamp", () => {
  it("carries the body and header through but stamps the message as OURS", () => {
    // What we publish is our identity, not the producer's — without the restamp the fleet cannot
    // tell who emitted a message, and the self-echo guard downstream cannot work either.
    const producer = Config.fromValue("com.example.Producer", "gw-09", {
      hierarchy: { levels: ["site", "device"] },
      identity: { site: "factory-1" },
      component: { global: {}, instances: [] },
    });
    const incoming: ProcMsg = {
      topic: "ecv1/gw-09/prod/main/data/temp",
      msg: MessageBuilder.create("Reading", "2.0").withPayload({ v: 42 }).withConfig(producer).build(),
    };

    const out = restamp(config, incoming);

    // Body + header name/version carried through unchanged.
    expect(out.body).toEqual({ v: 42 });
    expect(out.header.name).toBe("Reading");
    expect(out.header.version).toBe("2.0");
    // Identity restamped to us, not the producer.
    expect(out.identity?.path).toBe(config.componentIdentity.path);
    expect(out.identity?.component).toBe(config.componentIdentity.component);
    expect(isSelfEcho(out, config.componentIdentity.path, config.componentIdentity.component)).toBe(true);
  });
});
