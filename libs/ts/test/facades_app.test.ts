/**
 * Deterministic unit tests for {@link AppFacade} — the `app()` publish facade
 * (DESIGN-class-facades §2.3, D3): named header + verbatim body onto `app/{channel}`, minimal
 * enforcement, local/northbound routing (`stream` rejected). Mirrors the Java `AppFacadeTest`.
 */
import { describe, expect, it } from "vitest";

import { Config } from "../src/config/model";
import { EdgeCommonsError } from "../src/errors";
import { AppFacade } from "../src/facades/app_facade";
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
});
