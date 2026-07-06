import { describe, it, expect, beforeAll, afterEach } from "vitest";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

import { EdgeCommonsError } from "../src/errors";
import { loadMessagingConfig, lwtPayloadBytes, parseLwt, resolvedHost } from "../src/messaging/config";
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
    expect(cfg.iotCore).toBeUndefined();
    expect(resolvedHost(cfg.local)).toBe("localhost");
    expect(cfg.local.port).toBe(1883);
  });

  it("parses an iotCore broker section with defaults and credentials", async () => {
    const p = tmpFile(
      JSON.stringify({
        messaging: {
          local: { host: "localhost", credentials: { username: "u", password: "p" } },
          iotCore: { endpoint: "x.iot.amazonaws.com", credentials: { certPath: "c", keyPath: "k", caPath: "a" } },
        },
      }),
    );
    const cfg = await loadMessagingConfig(p);
    // default ports applied when omitted
    expect(cfg.local.port).toBe(1883);
    expect(cfg.iotCore?.port).toBe(8883);
    expect(resolvedHost(cfg.iotCore!)).toBe("x.iot.amazonaws.com");
    expect(cfg.local.credentials?.username).toBe("u");
  });

  it("throws when messaging.local is missing", async () => {
    const p = tmpFile(JSON.stringify({ messaging: {} }));
    await expect(loadMessagingConfig(p)).rejects.toBeInstanceOf(EdgeCommonsError);
    await loadMessagingConfig(p).catch((e) => expect((e as EdgeCommonsError).kind).toBe("Messaging"));
  });

  it("throws EdgeCommonsError(Io) when the file is missing", async () => {
    await expect(loadMessagingConfig("/no/such/file.json")).rejects.toBeInstanceOf(EdgeCommonsError);
  });

  it("resolvedHost prefers host, then endpoint, else throws", () => {
    expect(resolvedHost({ endpoint: "e.example", port: 8883, clientId: "x" })).toBe("e.example");
    expect(() => resolvedHost({ port: 1, clientId: "x" })).toThrow(EdgeCommonsError);
  });

  it("parses a messaging.lwt section (UNS-CANONICAL-DESIGN §6)", async () => {
    const p = tmpFile(
      JSON.stringify({
        messaging: {
          local: { host: "localhost", port: 1883, clientId: "c1" },
          lwt: { topic: "ecv1/gw-01/bridge/main/state", payload: { status: "UNREACHABLE" }, qos: 1 },
        },
      }),
    );
    const cfg = await loadMessagingConfig(p);
    expect(cfg.lwt).toEqual({
      topic: "ecv1/gw-01/bridge/main/state",
      payload: { status: "UNREACHABLE" },
      qos: 1,
    });
  });
});

describe("parseLwt / lwtPayloadBytes (§6)", () => {
  it("requires a topic; defaults qos to 1; coerces a lossless numeric qos", () => {
    expect(() => parseLwt({})).toThrow(EdgeCommonsError);
    expect(() => parseLwt({ payload: "x" })).toThrow(/lwt.topic is required/);
    expect(parseLwt({ topic: "t" }).qos).toBe(1);
    expect(parseLwt({ topic: "t", qos: 0 }).qos).toBe(0);
    expect(parseLwt({ topic: "t", qos: 1.0 }).qos).toBe(1); // JSON 1.0 parses to integer 1
    expect(() => parseLwt({ topic: "t", qos: 2 })).toThrow(/qos must be 0 or 1/);
    expect(() => parseLwt({ topic: "t", qos: "one" })).toThrow(/qos must be 0 or 1/);
  });

  it("serializes the payload: string verbatim, object as compact JSON, absent as empty", () => {
    expect(lwtPayloadBytes("offline").toString("utf8")).toBe("offline");
    expect(lwtPayloadBytes({ status: "UNREACHABLE" }).toString("utf8")).toBe('{"status":"UNREACHABLE"}');
    expect(lwtPayloadBytes(undefined).length).toBe(0);
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

  it("publishing to IoT Core without an iotCore broker throws", async (ctx) => {
    if (!up) ctx.skip();
    const cfg = await loadMessagingConfig(
      tmpFile(JSON.stringify({ messaging: { local: { host: "127.0.0.1", port: 1883, clientId: `ggc-it2-${Date.now()}` } } })),
    );
    const provider = await StandaloneMqttProvider.connect(cfg);
    // channel() throws synchronously (before the Promise is created).
    expect(() => provider.publishBytes("t", Buffer.from("x"), Destination.IoTCore, Qos.AtLeastOnce)).toThrow(EdgeCommonsError);
    await provider.disconnect();
  });

  it("registers the MQTT LWT at CONNECT: the broker publishes the will on an ungraceful drop (§6)", async (ctx) => {
    if (!up) ctx.skip();
    const willTopic = `ecv1/gw-lwt/${Math.random().toString(36).slice(2)}/main/state`;

    // Watcher connection (no LWT) subscribes to the will topic.
    const watcher = await StandaloneMqttProvider.connect(
      await loadMessagingConfig(
        tmpFile(JSON.stringify({ messaging: { local: { host: "127.0.0.1", port: 1883, clientId: `ggc-lwt-w-${Date.now()}` } } })),
      ),
    );
    const received: string[] = [];
    await watcher.subscribeRaw(willTopic, Destination.Local, Qos.AtLeastOnce, (_t, p) => {
      received.push(p.toString("utf8"));
    });

    // Victim connection registers the will (object payload -> compact JSON, retain=false).
    const victimCfg = await loadMessagingConfig(
      tmpFile(
        JSON.stringify({
          messaging: {
            local: { host: "127.0.0.1", port: 1883, clientId: `ggc-lwt-v-${Date.now()}` },
            lwt: { topic: willTopic, payload: { status: "UNREACHABLE" }, qos: 1 },
          },
        }),
      ),
    );
    const victim = await StandaloneMqttProvider.connect(victimCfg);

    // Force an UNGRACEFUL drop: destroy the underlying socket so no DISCONNECT packet is sent
    // and the broker fires the will. (A clean end() would suppress it.)
    const victimClient = (victim as unknown as { local: { client: { stream: { destroy(): void }; end(force: boolean): void } } })
      .local.client;
    victimClient.stream.destroy();

    for (let i = 0; i < 60 && received.length === 0; i++) await tick(100);
    expect(received).toEqual(['{"status":"UNREACHABLE"}']);

    victimClient.end(true);
    await watcher.disconnect();
  });
});
