import { describe, it, expect } from "vitest";
import { greengrasscoreipc } from "aws-iot-device-sdk-v2";

import { IpcMessagingProvider } from "../src/messaging/ipc-provider";
import { Destination, Qos } from "../src/messaging/types";
import { FakeIpcClient, tick } from "./_fakes";

import model = greengrasscoreipc.model;

function provider(client: FakeIpcClient): IpcMessagingProvider {
  return IpcMessagingProvider._withClient(
    client as unknown as greengrasscoreipc.Client,
    model.ReceiveMode.RECEIVE_MESSAGES_FROM_OTHERS,
  );
}

describe("IpcMessagingProvider (fake client)", () => {
  it("publishBytes local -> binaryMessage with the bytes", async () => {
    const client = new FakeIpcClient();
    const p = provider(client);
    const payload = Buffer.from("hello", "utf8");
    await p.publishBytes("local/topic", payload, Destination.Local, Qos.AtLeastOnce);
    expect(client.publishedTopic).toHaveLength(1);
    expect(client.publishedTopic[0].topic).toBe("local/topic");
    expect(client.publishedTopic[0].message.toString("utf8")).toBe("hello");
  });

  it("publishBytes iotcore -> publishToIoTCore", async () => {
    const client = new FakeIpcClient();
    const p = provider(client);
    await p.publishBytes("iot/topic", Buffer.from("x"), Destination.IoTCore, Qos.AtMostOnce);
    expect(client.publishedIot).toHaveLength(1);
    expect(client.publishedIot[0].topicName).toBe("iot/topic");
  });

  it("subscribeRaw local decodes binaryMessage and jsonMessage to bytes with context.topic", async () => {
    const client = new FakeIpcClient();
    const p = provider(client);
    const got: Array<{ topic: string; payload: string }> = [];
    await p.subscribeRaw("local/+", Destination.Local, Qos.AtLeastOnce, (topic, payload) => {
      got.push({ topic, payload: payload.toString("utf8") });
    });
    const stream = client.topicStreams[0];
    expect(stream.activated).toBe(true);

    stream.fire("message", {
      binaryMessage: { context: { topic: "local/a" }, message: Buffer.from("bin", "utf8") },
    });
    stream.fire("message", {
      jsonMessage: { context: { topic: "local/b" }, message: { k: 1 } },
    });
    await tick();
    expect(got[0]).toEqual({ topic: "local/a", payload: "bin" });
    expect(got[1].topic).toBe("local/b");
    expect(JSON.parse(got[1].payload)).toEqual({ k: 1 });
  });

  it("subscribeRaw iotcore decodes message.payload/topicName", async () => {
    const client = new FakeIpcClient();
    const p = provider(client);
    const got: Array<{ topic: string; payload: string }> = [];
    await p.subscribeRaw("iot/#", Destination.IoTCore, Qos.AtLeastOnce, (topic, payload) => {
      got.push({ topic, payload: payload.toString("utf8") });
    });
    client.iotStreams[0].fire("message", {
      message: { topicName: "iot/x", payload: Buffer.from("iotpayload", "utf8") },
    });
    await tick();
    expect(got[0]).toEqual({ topic: "iot/x", payload: "iotpayload" });
  });

  it("getConfiguration returns value; watchConfiguration fires onChange on an update event", async () => {
    const client = new FakeIpcClient();
    client.configValue = { z: 9 };
    const p = provider(client);
    expect(await p.getConfiguration(["K"], "comp")).toEqual({ z: 9 });

    let changes = 0;
    await p.watchConfiguration([], undefined, () => {
      changes++;
    });
    expect(client.configStreams[0].activated).toBe(true);
    client.configStreams[0].fire("message");
    client.configStreams[0].fire("message");
    expect(changes).toBe(2);
  });

  it("getThingShadow returns a Buffer; update/delete call through", async () => {
    const client = new FakeIpcClient();
    client.shadowBytes = Buffer.from("{\"state\":{}}", "utf8");
    const p = provider(client);
    const bytes = await p.getThingShadow("thing-1", "s");
    expect(Buffer.isBuffer(bytes)).toBe(true);
    expect(bytes.toString("utf8")).toBe('{"state":{}}');

    await p.updateThingShadow("thing-1", "s", Buffer.from("{}"));
    expect(client.shadowUpdates).toHaveLength(1);
    expect(client.shadowUpdates[0].thingName).toBe("thing-1");

    await p.deleteThingShadow("thing-1", "s");
    expect(client.shadowDeletes).toHaveLength(1);
  });

  it("disconnect closes tracked streams and the client", async () => {
    const client = new FakeIpcClient();
    const p = provider(client);
    await p.subscribeRaw("a/+", Destination.Local, Qos.AtLeastOnce, () => undefined);
    await p.subscribeRaw("b/#", Destination.IoTCore, Qos.AtLeastOnce, () => undefined);
    await p.disconnect();
    expect(client.topicStreams[0].closed).toBe(true);
    expect(client.iotStreams[0].closed).toBe(true);
    expect(client.closed).toBe(true);
  });
});
