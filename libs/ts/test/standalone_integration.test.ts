import { describe, it, expect, beforeAll, afterEach } from "vitest";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

import { EdgeCommonsError } from "../src/errors";
import { loadMessagingConfig, resolvedHost } from "../src/messaging/config";
import { StandaloneMqttProvider, topicMatches } from "../src/messaging/standalone-provider";
import { Destination, Qos } from "../src/messaging/types";
import { brokerReachable, tick } from "./_fakes";

const tmp: string[] = [];
function tmpFile(contents: string): string {
  const p = path.join(os.tmpdir(), `ggc-msg-${Math.random().toString(36).slice(2)}.json`);
  fs.writeFileSync(p, contents);
  tmp.push(p);
  return p;
}
afterEach(() => {
  for (const f of tmp.splice(0)) {
    try {
      fs.rmSync(f, { force: true });
    } catch {
      /* ignore */
    }
  }
});

describe("topicMatches unit", () => {
  it("matches exact, +, and # wildcards", () => {
    expect(topicMatches("a/b/c", "a/b/c")).toBe(true);
    expect(topicMatches("a/b/c", "a/b/d")).toBe(false);
    expect(topicMatches("a/+/c", "a/x/c")).toBe(true);
    expect(topicMatches("a/+/c", "a/x/y")).toBe(false);
    expect(topicMatches("a/#", "a/b/c/d")).toBe(true);
    expect(topicMatches("a/#", "a")).toBe(true); // multi-level '#' also matches the parent level
    expect(topicMatches("#", "anything/here")).toBe(true);
    expect(topicMatches("a/b", "a/b/c")).toBe(false);
  });
});

describe("loadMessagingConfig", () => {
  it("loads a local-only config and resolves the host", async () => {
    const p = tmpFile(JSON.stringify({ messaging: { local: { host: "localhost", port: 1883, clientId: "c1" } } }));
    const cfg = await loadMessagingConfig(p);
    expect(cfg.northbound).toBeUndefined();
    expect(resolvedHost(cfg.local)).toBe("localhost");
    expect(cfg.local.port).toBe(1883);
  });

  it("parses a northbound broker section with defaults and credentials", async () => {
    const p = tmpFile(
      JSON.stringify({
        messaging: {
            local: { host: "localhost", credentials: { username: "u", password: "p" } },
            northbound: { host: "broker.example.com", credentials: { certPath: "c", keyPath: "k", caPath: "a" } },
        },
      }),
    );
    const cfg = await loadMessagingConfig(p);
    // default ports applied when omitted
    expect(cfg.local.port).toBe(1883);
    expect(cfg.northbound?.port).toBe(8883);
    expect(resolvedHost(cfg.northbound!)).toBe("broker.example.com");
    expect(cfg.local.credentials?.username).toBe("u");
  });

  it("throws when messaging.local is missing", async () => {
    const p = tmpFile(JSON.stringify({ messaging: {} }));
    await expect(loadMessagingConfig(p)).rejects.toBeInstanceOf(EdgeCommonsError);
    await loadMessagingConfig(p).catch((e) => expect((e as EdgeCommonsError).kind).toBe("Messaging"));
  });

  it("rejects generic messaging.lwt", async () => {
    const p = tmpFile(
      JSON.stringify({
        messaging: {
          local: { host: "localhost", port: 1883, clientId: "c-lwt" },
          lwt: { topic: "ecv1/d/uns-bridge/main/state" },
        },
      }),
    );
    await expect(loadMessagingConfig(p)).rejects.toThrow(/messaging\.lwt is not supported/);
  });

  it("throws EdgeCommonsError(Io) when the file is missing", async () => {
    await expect(loadMessagingConfig("/no/such/file.json")).rejects.toBeInstanceOf(EdgeCommonsError);
  });

  it("resolvedHost prefers host, then endpoint, else throws", () => {
    expect(resolvedHost({ endpoint: "e.example", port: 8883, clientId: "x" })).toBe("e.example");
    expect(() => resolvedHost({ port: 1, clientId: "x" })).toThrow(EdgeCommonsError);
  });

  it("parses MQTT QoS defaults and rejects out-of-range northbound QoS", async () => {
    const p = tmpFile(
      JSON.stringify({
        messaging: {
          local: {
            host: "localhost",
            port: 1883,
            clientId: "c-qos",
            qos: { publish: 2, subscribe: 0 },
          },
          northbound: {
            host: "broker.example.com",
            port: 8883,
            clientId: "c-north-qos",
            qos: { publish: 2, subscribe: 0 },
          },
        },
      }),
    );
    const cfg = await loadMessagingConfig(p);
    expect(cfg.local.qos?.publish).toBe(Qos.ExactlyOnce);
    expect(cfg.local.qos?.subscribe).toBe(Qos.AtMostOnce);
    expect(cfg.northbound?.qos?.publish).toBe(Qos.ExactlyOnce);
    expect(cfg.northbound?.qos?.subscribe).toBe(Qos.AtMostOnce);

    const invalid = tmpFile(
      JSON.stringify({
        messaging: {
          local: { host: "localhost", port: 1883, clientId: "c-bad-qos" },
          northbound: { host: "broker.example.com", port: 8883, clientId: "c-bad-north", qos: { publish: 3 } },
        },
      }),
    );
    await expect(loadMessagingConfig(invalid)).rejects.toThrow(/messaging\.northbound\.qos\.publish/);

    const invalidType = tmpFile(
      JSON.stringify({
        messaging: {
          local: { host: "localhost", port: 1883, clientId: "c-bad-qos-type", qos: { subscribe: "1" } },
        },
      }),
    );
    await expect(loadMessagingConfig(invalidType)).rejects.toThrow(/messaging\.local\.qos\.subscribe/);
  });

  it("rejects top-level messaging.qos", async () => {
    const p = tmpFile(
      JSON.stringify({
        messaging: {
          local: { host: "localhost", port: 1883, clientId: "c-stale-qos" },
          qos: { local: { publish: 1 } },
        },
      }),
    );
    await expect(loadMessagingConfig(p)).rejects.toThrow(/messaging\.qos is not supported/);
  });
});

describe("StandaloneMqttProvider against the live broker", () => {
  let up = false;
  beforeAll(async () => {
    up = await brokerReachable();
  });

  it("connect -> publish/subscribe round-trip, unsubscribe, disconnect", async (ctx) => {
    if (!up) ctx.skip();
    const cfg = await loadMessagingConfig(
      tmpFile(
        JSON.stringify({
          messaging: { local: { host: "127.0.0.1", port: 1883, clientId: `ggc-it-${Date.now()}` } },
        }),
      ),
    );
    const provider = await StandaloneMqttProvider.connect(cfg);
    const topic = `ggc/it/${Math.random().toString(36).slice(2)}`;
    const received: string[] = [];
    const sub = await provider.subscribeRaw(topic, Destination.Local, Qos.AtLeastOnce, (_t, payload) => {
      received.push(payload.toString("utf8"));
    });

    await provider.publishBytes(topic, Buffer.from("payload-1", "utf8"), Destination.Local, Qos.AtLeastOnce);
    for (let i = 0; i < 40 && received.length === 0; i++) await tick(50);
    expect(received).toEqual(["payload-1"]);

    // Unsubscribe stops delivery.
    await sub.unsubscribe();
    await provider.publishBytes(topic, Buffer.from("payload-2", "utf8"), Destination.Local, Qos.AtLeastOnce);
    await tick(300);
    expect(received).toEqual(["payload-1"]);

    await provider.disconnect();
  });

  it("connects with local credentials supplied (username/password)", async (ctx) => {
    if (!up) ctx.skip();
    // EMQX allow_anonymous accepts these; this exercises the credentials branch.
    const cfg = await loadMessagingConfig(
      tmpFile(
        JSON.stringify({
          messaging: {
            local: {
              host: "127.0.0.1",
              port: 1883,
              clientId: `ggc-cred-${Date.now()}`,
              credentials: { username: "tester", password: "secret" },
            },
          },
        }),
      ),
    );
    const provider = await StandaloneMqttProvider.connect(cfg);
    await provider.disconnect();
  });

  it("connect rejects when the broker is unreachable", async () => {
    const cfg = await loadMessagingConfig(
      tmpFile(JSON.stringify({ messaging: { local: { host: "127.0.0.1", port: 1, clientId: "ggc-bad" } } })),
    );
    await expect(StandaloneMqttProvider.connect(cfg)).rejects.toBeInstanceOf(EdgeCommonsError);
  });

  it("publishing to IoT Core API without a northbound broker throws", async (ctx) => {
    if (!up) ctx.skip();
    const cfg = await loadMessagingConfig(
      tmpFile(JSON.stringify({ messaging: { local: { host: "127.0.0.1", port: 1883, clientId: `ggc-it2-${Date.now()}` } } })),
    );
    const provider = await StandaloneMqttProvider.connect(cfg);
    // channel() throws synchronously (before the Promise is created).
    expect(() => provider.publishBytes("t", Buffer.from("x"), Destination.Northbound, Qos.AtLeastOnce)).toThrow(EdgeCommonsError);
    await provider.disconnect();
  });

});
