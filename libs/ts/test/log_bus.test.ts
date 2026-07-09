import { afterEach, describe, expect, it, vi } from "vitest";

import { Config } from "../src/config/model";
import { logger, initLogging } from "../src/logging";
import { LogBusService } from "../src/log_bus";
import { MessageBuilder } from "../src/message";
import { DefaultMessagingService } from "../src/messaging/service";
import { ReservedTopicError } from "../src/messaging/types";
import { RecordingMessagingService, FakeMessagingProvider, tick } from "./_fakes";

afterEach(() => {
  initLogging(Config.fromValue("c", "t", { logging: { level: "INFO" } }));
  vi.restoreAllMocks();
});

function cfg(raw: Record<string, unknown>): Config {
  return Config.fromValue("com.example.C", "thing-1", raw);
}

describe("LogBusService", () => {
  it("publishes explicit records on the reserved log path with the required envelope/body", async () => {
    const config = cfg({
      component: {},
      logging: { publish: { enabled: true, captureNative: false } },
    });
    const messaging = new RecordingMessagingService();
    const logs = new LogBusService(() => config, messaging);

    await logs.publish({
      timestamp: "2026-07-09T12:34:56.789Z",
      level: "warn",
      logger: "app.worker",
      message: "pump degraded",
      sequence: 42,
      thread: "main",
      fields: { station: "A1" },
      error: "low pressure",
    });
    await logs.flush();

    expect(messaging.published.length).toBe(1);
    const call = messaging.published[0];
    expect(call.kind).toBe("publishReserved");
    expect(call.topic).toBe("ecv1/thing-1/C/main/log/warn");
    expect(call.message?.header.name).toBe("log");
    expect(call.message?.header.version).toBe("1.0");
    expect(call.message?.header.timestamp).toBe("2026-07-09T12:34:56.789Z");
    expect(call.message?.identity?.instance).toBe("main");
    expect(call.message?.body).toEqual({
      schema: "edgecommons.log.v1",
      timestamp: "2026-07-09T12:34:56.789Z",
      level: "WARN",
      logger: "app.worker",
      message: "pump degraded",
      sequence: 42,
      thread: "main",
      fields: { station: "A1" },
      error: "low pressure",
    });
    logs.close();
  });

  it("routes northbound through the reserved northbound seam", async () => {
    const config = cfg({
      component: {},
      logging: { publish: { enabled: true, destination: "northbound", captureNative: false } },
    });
    const messaging = new RecordingMessagingService();
    const logs = new LogBusService(() => config, messaging);

    await logs.publish({ timestamp: 0, level: "ERROR", logger: "app", message: "boom" });
    await logs.flush();

    expect(messaging.published[0].kind).toBe("publishReservedNorthbound");
    expect(messaging.published[0].topic).toBe("ecv1/thing-1/C/main/log/error");
    logs.close();
  });

  it("does not call the reserved transport when messaging is disconnected", async () => {
    const config = cfg({
      component: {},
      logging: { publish: { enabled: true, captureNative: false } },
    });
    const messaging = new RecordingMessagingService();
    messaging.connectedState = false;
    const logs = new LogBusService(() => config, messaging);

    await expect(logs.publish({ level: "ERROR", logger: "app", message: "offline" })).rejects.toThrow(
      /disconnected/,
    );

    expect(messaging.published).toEqual([]);
    expect(logs.stats().failed).toBe(1);
    logs.close();
  });

  it("does not recapture logger records emitted inside reserved publish", async () => {
    class LoggingMessaging extends RecordingMessagingService {
      override async publishReserved(
        topic: string,
        msg: NonNullable<RecordingMessagingService["published"][number]["message"]>,
      ): Promise<void> {
        logger.error("provider publish warning should not recurse");
        await super.publishReserved(topic, msg);
      }
    }

    const config = cfg({
      component: {},
      logging: { level: "TRACE", publish: { enabled: true, captureNative: true } },
    });
    const messaging = new LoggingMessaging();
    const logs = new LogBusService(() => config, messaging);
    initLogging(config);
    vi.spyOn(process.stderr, "write").mockImplementation(() => true);

    await logs.publish({ level: "INFO", logger: "app", message: "one" });
    await logs.flush();

    expect(messaging.published.length).toBe(1);
    expect(messaging.published[0].message?.body).toMatchObject({ message: "one" });
    logs.close();
  });

  it("redacts sensitive fields and extra patterns before publishing", async () => {
    const config = cfg({
      component: {},
      logging: {
        publish: {
          enabled: true,
          captureNative: false,
          redaction: { replacement: "[redacted]", extraPatterns: ["secret-[0-9]+"] },
        },
      },
    });
    const messaging = new RecordingMessagingService();
    const logs = new LogBusService(() => config, messaging);

    await logs.publish({
      level: "INFO",
      logger: "app",
      message: "token secret-123",
      fields: { password: "clear", nested: { apiKey: "abc" } },
    });
    await logs.flush();

    expect(messaging.published[0].message?.body).toMatchObject({
      message: "token [redacted]",
      fields: { password: "[redacted]", nested: { apiKey: "[redacted]" } },
    });
    expect(logs.stats().redacted).toBe(3);
    logs.close();
  });

  it("truncates over-sized records and records truncation stats", async () => {
    const config = cfg({
      component: {},
      logging: { publish: { enabled: true, captureNative: false, maxRecordBytes: 180 } },
    });
    const messaging = new RecordingMessagingService();
    const logs = new LogBusService(() => config, messaging);

    await logs.publish({
      timestamp: "2026-07-09T00:00:00.000Z",
      level: "INFO",
      logger: "app",
      message: "x".repeat(500),
    });
    await logs.flush();

    const body = messaging.published[0].message?.body as Record<string, unknown>;
    expect(body.truncated).toBe(true);
    expect(String(body.message).endsWith("...")).toBe(true);
    expect(Buffer.byteLength(JSON.stringify(body), "utf8")).toBeLessThanOrEqual(180);
    expect(logs.stats().truncated).toBe(1);
    logs.close();
  });

  it("drops the oldest queued record on overflow and annotates the next published record", async () => {
    class BlockingMessaging extends RecordingMessagingService {
      releaseFirst!: () => void;
      private blocked = false;
      override async publishReserved(topic: string, msg: NonNullable<RecordingMessagingService["published"][number]["message"]>): Promise<void> {
        if (!this.blocked) {
          this.blocked = true;
          await new Promise<void>((resolve) => {
            this.releaseFirst = resolve;
          });
        }
        await super.publishReserved(topic, msg);
      }
    }

    const config = cfg({
      component: {},
      logging: {
        publish: { enabled: true, captureNative: false, queue: { maxRecords: 2, onFull: "dropOldest" } },
      },
    });
    const messaging = new BlockingMessaging();
    const logs = new LogBusService(() => config, messaging);

    const p1 = logs.publish({ level: "INFO", logger: "app", message: "first" });
    await tick();
    const p2 = logs.publish({ level: "INFO", logger: "app", message: "dropped" });
    const p3 = logs.publish({ level: "INFO", logger: "app", message: "kept-1" });
    const p4 = logs.publish({ level: "INFO", logger: "app", message: "kept-2" });

    messaging.releaseFirst();
    await Promise.all([p1, p2, p3, p4]);
    await logs.flush();

    const bodies = messaging.published.map((p) => p.message?.body as Record<string, unknown>);
    expect(bodies.map((b) => b.message)).toEqual(["first", "kept-1", "kept-2"]);
    expect(bodies[1].dropped).toBe(1);
    expect(logs.stats().dropped).toBe(1);
    logs.close();
  });

  it("captures EdgeCommons Logger records when captureNative is enabled", async () => {
    const config = cfg({
      component: {},
      logging: { level: "INFO", publish: { enabled: true, captureNative: true } },
    });
    const messaging = new RecordingMessagingService();
    const logs = new LogBusService(() => config, messaging);
    initLogging(config);
    vi.spyOn(process.stdout, "write").mockImplementation(() => true);

    logger.info("captured");
    await logs.flush();

    expect(messaging.published.length).toBe(1);
    expect(messaging.published[0].topic).toBe("ecv1/thing-1/C/main/log/info");
    expect(messaging.published[0].message?.body).toMatchObject({
      schema: "edgecommons.log.v1",
      level: "INFO",
      logger: "edgecommons",
      message: "captured",
      sequence: 1,
    });
    logs.close();
  });

  it("keeps raw public publishes to the reserved log class rejected", async () => {
    const provider = new FakeMessagingProvider();
    const svc = new DefaultMessagingService(provider);
    const topic = "ecv1/thing-1/C/main/log/info";
    const msg = MessageBuilder.create("log", "1.0").withPayload({}).build();

    await expect(svc.publish(topic, msg)).rejects.toBeInstanceOf(ReservedTopicError);
    await expect(svc.publishRaw(topic, {})).rejects.toBeInstanceOf(ReservedTopicError);
    await svc.publishReserved(topic, msg);
    expect(provider.published[0].topic).toBe(topic);
  });
});
