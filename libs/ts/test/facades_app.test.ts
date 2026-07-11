/**
 * Deterministic unit tests for {@link AppFacade} — the `app()` publish facade
 * (DESIGN-class-facades §2.3, D3): named header + verbatim body onto `app/{channel}`, minimal
 * enforcement, local/northbound routing (`stream` rejected). Mirrors the Java `AppFacadeTest`.
 */
import { describe, expect, it } from "vitest";

import { Config } from "../src/config/model";
import { EdgeCommonsError } from "../src/errors";
import { AppFacade } from "../src/facades/app_facade";
import { Message, MessageBuilder } from "../src/message";
import { Channel } from "../src/facades/channel";
import { Qos } from "../src/messaging/types";
import { Uns } from "../src/uns";
import { RecordingMessagingService } from "./_fakes";

function makeFacade(): { facade: AppFacade; messaging: RecordingMessagingService } {
  const cfg = Config.fromValue("opcua-adapter", "gw-01", { component: {} });
  const messaging = new RecordingMessagingService();
  const uns = new Uns(cfg.componentIdentity, false);
  const facade = new AppFacade(() => cfg, "main", uns, messaging);
  return { facade, messaging };
}

describe("AppFacade", () => {
  it("publishes the body verbatim with the caller's header name", async () => {
    const { facade, messaging } = makeFacade();
    await facade.publish("OrderReceived", "order/received", { orderId: "A-42", qty: 3 });

    const rec = messaging.published[0];
    expect(rec.topic).toBe("ecv1/gw-01/opcua-adapter/main/app/order/received");
    expect(rec.message!.header.name).toBe("OrderReceived");
    expect(rec.message!.getBody()).toEqual({ orderId: "A-42", qty: 3 });
    expect(rec.qos, "LOCAL is the default").toBeUndefined();
  });

  it("sanitizes each channel token", async () => {
    const { facade, messaging } = makeFacade();
    await facade.publish("Ping", "a+b", { n: 1 });
    expect(messaging.published[0].topic).toBe("ecv1/gw-01/opcua-adapter/main/app/a_b");
  });

  it("an empty body publishes as {}", async () => {
    const { facade, messaging } = makeFacade();
    await facade.publish("Beat", "beat", {});
    expect(messaging.published[0].message!.getBody()).toEqual({});
  });

  it("rejects an empty name or channel", async () => {
    const { facade } = makeFacade();
    await expect(facade.publish("", "hello", {})).rejects.toThrow(EdgeCommonsError);
    await expect(facade.publish("Hello", "", {})).rejects.toThrow(EdgeCommonsError);
  });

  it("routes northbound on an explicit override", async () => {
    const { facade, messaging } = makeFacade();
    await facade.publish("CloudEvent", "cloud", { k: "v" }, Channel.NORTHBOUND);
    expect(messaging.published[0].qos).toBe(Qos.AtLeastOnce);
  });

  it("rejects a stream routing override", async () => {
    const { facade } = makeFacade();
    await expect(facade.publish("Ping", "hello", {}, Channel.stream("hot"))).rejects.toThrow(EdgeCommonsError);
  });

  it("a northbound transport failure is swallowed (readiness stays local)", async () => {
    const { facade, messaging } = makeFacade();
    messaging.publishNorthbound = async () => {
      throw new Error("iot core down");
    };
    await expect(facade.publish("CloudEvent", "cloud", { k: "v" }, Channel.NORTHBOUND)).resolves.toBeUndefined();
  });

  it("prepare captures topic, envelope, and defensive exact bytes", () => {
    const { facade } = makeFacade();
    const prepared = facade.prepare("ImageCaptured", "image/captured", { captureId: "cap-1" });
    const original = prepared.encodedBytes;
    const mutated = prepared.encodedBytes;
    mutated[0] ^= 0x7f;

    expect(prepared.topic).toBe("ecv1/gw-01/opcua-adapter/main/app/image/captured");
    expect(prepared.encodedBytes.equals(original)).toBe(true);
    const decoded = Message.fromBytes(original);
    expect(prepared.message.header.uuid).toBe(decoded.header.uuid);
    expect(decoded.getBody()).toEqual({ captureId: "cap-1" });
  });

  it("prepareCorrelated accepts a received request or explicit correlation id", () => {
    const { facade } = makeFacade();
    const request = MessageBuilder.create("sb/capture", "1.0")
      .withCorrelationId("corr-request")
      .withPayload({})
      .build();
    expect(
      facade.prepareCorrelated("ImageCaptured", "image/captured", {}, request).message.header.correlation_id,
    ).toBe("corr-request");
    expect(
      facade.prepareCorrelated("ImageCaptured", "image/captured", {}, "corr-explicit").message.header.correlation_id,
    ).toBe("corr-explicit");
    expect(() => facade.prepareCorrelated("X", "x", {}, "")).toThrow(EdgeCommonsError);
    expect(() => facade.prepareCorrelated("X", "x", {}, Message.fromObject({}))).toThrow(EdgeCommonsError);
  });

  it("confirmed prepared publish uses exact bytes, QoS 1, and routing", async () => {
    const { facade, messaging } = makeFacade();
    const prepared = facade.prepareCorrelated("ImageCaptured", "image/captured", {}, "corr-1");
    const exact = prepared.encodedBytes;
    prepared.message.header.correlation_id = "mutated-view";

    await facade.publishConfirmed(prepared, 250);
    expect(messaging.published[0]).toMatchObject({
      kind: "publishConfirmed",
      topic: prepared.topic,
      qos: Qos.AtLeastOnce,
      timeoutMs: 250,
    });
    expect(messaging.published[0].encodedBytes!.equals(exact)).toBe(true);

    messaging.published.length = 0;
    await facade.publishConfirmed(prepared, 300, Channel.NORTHBOUND);
    expect(messaging.published[0]).toMatchObject({
      kind: "publishNorthboundConfirmed",
      qos: Qos.AtLeastOnce,
      timeoutMs: 300,
    });
    expect(messaging.published[0].encodedBytes!.equals(exact)).toBe(true);
  });

  it("confirmed prepared publish rejects unsupported services and stream routing", async () => {
    const { facade, messaging } = makeFacade();
    const prepared = facade.prepare("X", "x", {});
    messaging.publishConfirmed = undefined;
    await expect(facade.publishConfirmed(prepared, 100)).rejects.toMatchObject({ reason: "unsupported" });
    await expect(facade.publishConfirmed(prepared, 100, Channel.stream("hot"))).rejects.toThrow(EdgeCommonsError);
  });
});
