/**
 * Deterministic unit tests for {@link EventsFacade} — the `events()` publish facade
 * (DESIGN-class-facades §2.2, D8): the `evt/{severity}/{type}` channel DERIVED from the body, the
 * `timestamp` → now default, `raiseAlarm`/`clearAlarm` alarm/active fields, and the
 * local/northbound routing (`stream` rejected). Mirrors the Java `EventsFacadeTest`.
 */
import { describe, expect, it } from "vitest";

import { Config } from "../src/config/model";
import { EdgeCommonsError } from "../src/errors";
import { Channel } from "../src/facades/channel";
import { EventsFacade } from "../src/facades/events_facade";
import { Severity } from "../src/facades/severity";
import { Qos } from "../src/messaging/types";
import { Uns } from "../src/uns";
import { RecordingMessagingService } from "./_fakes";

const NOW = "2026-07-01T12:00:00Z";
const FIXED_CLOCK = (): number => Date.parse(NOW);

function makeFacade(): { facade: EventsFacade; messaging: RecordingMessagingService } {
  const cfg = Config.fromValue("opcua-adapter", "gw-01", { component: {} });
  const messaging = new RecordingMessagingService();
  const uns = new Uns(cfg.componentIdentity, false);
  const facade = new EventsFacade(() => cfg, "main", uns, messaging, FIXED_CLOCK);
  return { facade, messaging };
}

describe("EventsFacade", () => {
  it("emit derives the evt/{severity}/{type} channel from the body", async () => {
    const { facade, messaging } = makeFacade();
    await facade.emit(Severity.Critical, "overtemp", "sensor over threshold", { celsius: 95.0 });

    const rec = messaging.published[0];
    expect(rec.topic).toBe("ecv1/gw-01/opcua-adapter/main/evt/critical/overtemp");
    const body = rec.message!.getBody() as Record<string, unknown>;
    expect(body.severity).toBe("critical");
    expect(body.type).toBe("overtemp");
    expect(body.message).toBe("sensor over threshold");
    expect(body.timestamp).toBe(NOW);
    expect(body.context).toEqual({ celsius: 95.0 });
    expect(body.alarm, "a plain emit carries no alarm/active").toBeUndefined();
  });

  it("emitInfo defaults severity to info", async () => {
    const { facade, messaging } = makeFacade();
    await facade.emitInfo("door-open", "front door opened");

    const rec = messaging.published[0];
    expect(rec.topic).toBe("ecv1/gw-01/opcua-adapter/main/evt/info/door-open");
    expect((rec.message!.getBody() as Record<string, unknown>).severity).toBe("info");
  });

  it("every severity token is a valid channel segment", async () => {
    const { facade, messaging } = makeFacade();
    await facade.emit(Severity.Warning, "write-rejected", "write not in allow-list");
    await facade.emit(Severity.Debug, "poll-cycle");

    expect(messaging.published[0].topic).toBe("ecv1/gw-01/opcua-adapter/main/evt/warning/write-rejected");
    expect(messaging.published[1].topic).toBe("ecv1/gw-01/opcua-adapter/main/evt/debug/poll-cycle");
    expect((messaging.published[1].message!.getBody() as Record<string, unknown>).message).toBeUndefined();
  });

  it("the event type is sanitized into the channel (body keeps the raw type)", async () => {
    const { facade, messaging } = makeFacade();
    await facade.emit(Severity.Info, "a+b", "type sanitized for the channel");

    expect(messaging.published[0].topic).toBe("ecv1/gw-01/opcua-adapter/main/evt/info/a_b");
    expect((messaging.published[0].message!.getBody() as Record<string, unknown>).type).toBe("a+b");
  });

  it("raiseAlarm defaults severity to critical and sets alarm=true, active=true", async () => {
    const { facade, messaging } = makeFacade();
    await facade.raiseAlarm("connection-lost", "modbus link down", { connected: false });

    const rec = messaging.published[0];
    expect(rec.topic).toBe("ecv1/gw-01/opcua-adapter/main/evt/critical/connection-lost");
    const body = rec.message!.getBody() as Record<string, unknown>;
    expect(body.alarm).toBe(true);
    expect(body.active).toBe(true);
    expect(body.context).toEqual({ connected: false });
  });

  it("clearAlarm defaults severity to critical and sets alarm=true, active=false", async () => {
    const { facade, messaging } = makeFacade();
    await facade.clearAlarm("connection-lost");

    const rec = messaging.published[0];
    expect(rec.topic, "clear rides the same channel as the raise").toBe("ecv1/gw-01/opcua-adapter/main/evt/critical/connection-lost");
    const body = rec.message!.getBody() as Record<string, unknown>;
    expect(body.alarm).toBe(true);
    expect(body.active).toBe(false);
    expect(body.message, "clearAlarm has no message").toBeUndefined();
  });

  it("raiseAlarm/clearAlarm honor an explicit severity override", async () => {
    const { facade, messaging } = makeFacade();
    await facade.raiseAlarm("degraded", "running degraded", undefined, Severity.Warning);

    expect(messaging.published[0].topic).toBe("ecv1/gw-01/opcua-adapter/main/evt/warning/degraded");
  });

  it("a non-empty type is required", async () => {
    const { facade } = makeFacade();
    await expect(facade.emit(Severity.Info, "")).rejects.toThrow(EdgeCommonsError);
  });

  describe("routing", () => {
    it("via(NORTHBOUND) routes to IoT Core", async () => {
      const { facade, messaging } = makeFacade();
      await facade.via(Channel.NORTHBOUND).emit(Severity.Critical, "overtemp", "escalate to cloud");

      expect(messaging.published[0].qos).toBe(Qos.AtLeastOnce);
    });

    it("via(stream) is rejected - events are not bulk telemetry", () => {
      const { facade } = makeFacade();
      expect(() => facade.via(Channel.stream("hot"))).toThrow(EdgeCommonsError);
    });

    it("a northbound transport failure is swallowed (readiness stays local)", async () => {
      const { facade, messaging } = makeFacade();
      messaging.publishToIoTCore = async () => {
        throw new Error("iot core down");
      };
      await expect(facade.via(Channel.NORTHBOUND).emit(Severity.Critical, "overtemp")).resolves.toBeUndefined();
    });
  });
});
