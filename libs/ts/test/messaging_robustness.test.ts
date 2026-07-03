/**
 * Robustness tests mirroring the Java fix (commit 6ed774c):
 *
 *  1. A late/duplicate reply (delivered after the request has settled and its
 *     reply subscription has been torn down) must not throw or reject — it is
 *     logged + dropped.
 *  2. A subscription callback that throws (or, for async callbacks, rejects) must
 *     be contained by the dispatcher / IPC provider — the provider does not throw,
 *     no unhandledRejection escapes, and the subscription stays usable.
 */
import { describe, it, expect, vi, afterEach } from "vitest";
import { greengrasscoreipc } from "aws-iot-device-sdk-v2";

import { IpcMessagingProvider } from "../src/messaging/ipc-provider";
import { DefaultMessagingService } from "../src/messaging/service";
import { Destination, MessagingProvider, Qos, RawSubscription } from "../src/messaging/types";
import { MessageBuilder } from "../src/message";
import { FakeIpcClient, FakeMessagingProvider, tick } from "./_fakes";

import model = greengrasscoreipc.model;

function ipcProvider(client: FakeIpcClient): IpcMessagingProvider {
  return IpcMessagingProvider._withClient(
    client as unknown as greengrasscoreipc.Client,
    model.ReceiveMode.RECEIVE_MESSAGES_FROM_OTHERS,
  );
}

/** Track unhandled rejections for the duration of a test. */
function trapUnhandled(): { rejections: unknown[]; restore: () => void } {
  const rejections: unknown[] = [];
  const onUnhandled = (reason: unknown): void => {
    rejections.push(reason);
  };
  process.on("unhandledRejection", onUnhandled);
  return {
    rejections,
    restore: () => process.off("unhandledRejection", onUnhandled),
  };
}

describe("Fix 1: late/duplicate reply is dropped, not crashed", () => {
  afterEach(() => vi.restoreAllMocks());

  it("delivering a reply after the request settled does not throw or reject", async () => {
    const provider = new FakeMessagingProvider();
    const svc = new DefaultMessagingService(provider);
    const trap = trapUnhandled();

    // Responder replies exactly once.
    await svc.subscribe("rpc/once", async (_t, req) => {
      const reply = MessageBuilder.create("reply", "1.0.0").withPayload({ ok: true }).build();
      await svc.reply(req, reply);
    });

    const req = MessageBuilder.create("ask", "1.0.0").withPayload({ q: 1 }).build();
    const reply = await svc.request("rpc/once", req, 1000);
    expect(reply.getBody()).toEqual({ ok: true });

    // `request()` populated reply_to with the generated reply topic; the reply
    // subscription on it has since been torn down.
    const replyTopic = req.header.reply_to;
    expect(replyTopic).toBeTruthy();

    // Simulate a STRAGGLER reply on that same reply topic (a late/duplicate). It
    // must be a no-op: no throw, no reject — there is no pending resolver left.
    const stray = MessageBuilder.create("reply", "1.0.0").withPayload({ ok: "late" }).build();
    await expect(
      provider.publishBytes(replyTopic!, Buffer.from(stray.toJSON(), "utf8"), Destination.Local, Qos.AtLeastOnce),
    ).resolves.toBeUndefined();

    await tick(5);
    expect(trap.rejections).toEqual([]);
    trap.restore();

    // The service is still usable for a fresh request.
    await svc.subscribe("rpc/again", async (_t, r) => {
      await svc.reply(r, MessageBuilder.create("reply", "1.0.0").withPayload({ n: 2 }).build());
    });
    const r2 = await svc.request("rpc/again", MessageBuilder.create("ask", "1.0.0").withPayload({}).build(), 1000);
    expect(r2.getBody()).toEqual({ n: 2 });
  });
});

describe("Fix 2: a throwing/rejecting subscription callback is contained", () => {
  afterEach(() => vi.restoreAllMocks());

  it("IPC provider local subscription: a synchronously throwing callback does not escape and the stream stays usable", async () => {
    const client = new FakeIpcClient();
    const p = ipcProvider(client);
    const got: string[] = [];

    await p.subscribeRaw("local/+", Destination.Local, Qos.AtLeastOnce, (_topic, payload) => {
      const text = payload.toString("utf8");
      if (text === "boom") throw new Error("callback blew up");
      got.push(text);
    });
    const stream = client.topicStreams[0];

    // A bad message must NOT propagate out of the eventstream `message` handler.
    expect(() =>
      stream.fire("message", { binaryMessage: { context: { topic: "local/a" }, message: Buffer.from("boom", "utf8") } }),
    ).not.toThrow();

    // The subscription is still usable: a subsequent good message is delivered.
    stream.fire("message", { binaryMessage: { context: { topic: "local/b" }, message: Buffer.from("ok", "utf8") } });
    expect(got).toEqual(["ok"]);
  });

  it("IPC provider IoT Core subscription: a throwing callback is contained", async () => {
    const client = new FakeIpcClient();
    const p = ipcProvider(client);
    const got: string[] = [];

    await p.subscribeRaw("iot/#", Destination.IoTCore, Qos.AtLeastOnce, (_t, payload) => {
      if (payload.toString("utf8") === "boom") throw new Error("iot callback blew up");
      got.push(payload.toString("utf8"));
    });
    const stream = client.iotStreams[0];

    expect(() =>
      stream.fire("message", { message: { topicName: "iot/x", payload: Buffer.from("boom", "utf8") } }),
    ).not.toThrow();
    stream.fire("message", { message: { topicName: "iot/y", payload: Buffer.from("ok", "utf8") } });
    expect(got).toEqual(["ok"]);
  });

  it("IPC provider: an async callback that REJECTS is caught (no unhandledRejection)", async () => {
    const client = new FakeIpcClient();
    const p = ipcProvider(client);
    const trap = trapUnhandled();

    await p.subscribeRaw("local/+", Destination.Local, Qos.AtLeastOnce, async (_t, payload) => {
      if (payload.toString("utf8") === "boom") throw new Error("async rejection");
    });
    const stream = client.topicStreams[0];

    expect(() =>
      stream.fire("message", { binaryMessage: { context: { topic: "local/a" }, message: Buffer.from("boom", "utf8") } }),
    ).not.toThrow();

    await tick(5);
    expect(trap.rejections).toEqual([]);
    trap.restore();
  });

  it("service Dispatcher: a throwing message handler is contained and processing continues", async () => {
    // Exercise the DefaultMessagingService dispatch path with a handler that throws
    // on the first message; the second message must still be delivered.
    const provider = new FakeMessagingProvider();
    const svc = new DefaultMessagingService(provider);
    const trap = trapUnhandled();
    const seen: number[] = [];

    await svc.subscribe(
      "evt/x",
      (_t, m) => {
        const n = (m.getBody() as { n: number }).n;
        if (n === 1) throw new Error("handler blew up");
        seen.push(n);
      },
      32,
      1,
    );

    await svc.publish("evt/x", MessageBuilder.create("e", "1").withPayload({ n: 1 }).build());
    await svc.publish("evt/x", MessageBuilder.create("e", "1").withPayload({ n: 2 }).build());
    await tick(10);

    expect(seen).toEqual([2]);
    expect(trap.rejections).toEqual([]);
    trap.restore();
  });
});
