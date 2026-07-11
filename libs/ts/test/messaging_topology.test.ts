/**
 * Phase 1b messaging-topology tests (FR-MSG-2 / FR-MSG-3).
 *
 * These exercise {@link StandaloneMqttProvider.connect} with the `mqtt` module mocked, so the
 * single- vs dual-broker wiring and the per-broker TLS/URL decisions are verified deterministically
 * without a live broker:
 *
 *  - FR-MSG-2: a Kubernetes Service-DNS host (`emqx.mqtt.svc.cluster.local`) is an opaque string and
 *    is used verbatim in the broker URL (no special handling, no insecure behavior).
 *  - FR-MSG-3: `messaging.northbound` presence selects the topology — single broker (air-gapped,
 *    local only) when absent, dual (local + northbound MQTT) when present. The northbound leg is a
 *    generic MQTT connection: plaintext unless a CA is configured, TLS when `caPath` is present.
 *
 * FR-MSG-4 (the message envelope) is untouched — these tests do not serialize/parse envelopes.
 */
import { describe, it, expect, vi, afterEach, beforeEach } from "vitest";
import { EventEmitter } from "events";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

const hoisted = vi.hoisted(() => ({
  connectCalls: [] as { url: string; options: Record<string, unknown> }[],
  clients: [] as Array<EventEmitter & Record<string, unknown>>,
  publishCalls: [] as Array<{ topic: string; payload: Buffer; options: Record<string, unknown> }>,
  publishCallbacks: [] as Array<(error?: Error | null) => void>,
  deferPublishAck: false,
}));

vi.mock("mqtt", () => {
  const connect = (url: string, options: Record<string, unknown>): EventEmitter => {
    hoisted.connectCalls.push({ url, options });
    const client = new EventEmitter() as EventEmitter & Record<string, unknown>;
    client.connected = true;
    client.publish = (
      topic: string,
      payload: Buffer,
      publishOptions: Record<string, unknown>,
      cb?: (e?: Error | null) => void,
    ): void => {
      hoisted.publishCalls.push({ topic, payload: Buffer.from(payload), options: publishOptions });
      if (!cb) return;
      if (hoisted.deferPublishAck) hoisted.publishCallbacks.push(cb);
      else cb();
    };
    client.subscribe = (_f: string, _o: unknown, cb?: (e?: Error) => void): void => cb?.();
    client.unsubscribe = (_f: string, cb?: () => void): void => cb?.();
    client.end = (_force?: unknown, _o?: unknown, cb?: () => void): void => cb?.();
    hoisted.clients.push(client);
    // Emit CONNACK asynchronously so connectBroker's once("connect") handler is registered first.
    setImmediate(() => client.emit("connect"));
    return client;
  };
  return { default: { connect }, connect };
});

import { StandaloneMqttProvider } from "../src/messaging/standalone-provider";
import { loadMessagingConfig } from "../src/messaging/config";
import { Destination, Qos } from "../src/messaging/types";
import { EdgeCommonsError } from "../src/errors";

const tmp: string[] = [];
function tmpFile(name: string, contents: string): string {
  const p = path.join(os.tmpdir(), `ggc-topo-${Math.random().toString(36).slice(2)}-${name}`);
  fs.writeFileSync(p, contents);
  tmp.push(p);
  return p;
}

beforeEach(() => {
  hoisted.connectCalls.length = 0;
  hoisted.clients.length = 0;
  hoisted.publishCalls.length = 0;
  hoisted.publishCallbacks.length = 0;
  hoisted.deferPublishAck = false;
});

afterEach(() => {
  for (const f of tmp.splice(0)) {
    try {
      fs.rmSync(f, { force: true });
    } catch {
      /* ignore */
    }
  }
  vi.restoreAllMocks();
});

describe("FR-MSG-3: single-broker (air-gapped) topology", () => {
  it("connects only the local broker when messaging.northbound is absent", async () => {
    const cfg = await loadMessagingConfig(
      tmpFile(
        "single.json",
        JSON.stringify({ messaging: { local: { host: "localhost", port: 1883, clientId: "c-local" } } }),
      ),
    );
    expect(cfg.northbound).toBeUndefined(); // the loader's topology decision: single

    const provider = await StandaloneMqttProvider.connect(cfg);
    expect(hoisted.connectCalls).toHaveLength(1); // exactly one broker connection
    expect(hoisted.connectCalls[0].url).toBe("mqtt://localhost:1883");

    // No northbound channel exists: publishing to the second broker throws synchronously.
    expect(() =>
      provider.publishBytes("t", Buffer.from("x"), Destination.Northbound, Qos.AtLeastOnce),
    ).toThrow(EdgeCommonsError);
    await provider.disconnect();
  });
});

describe("FR-MSG-3: dual-MQTT topology + northbound broker", () => {
  it("connects both brokers when messaging.northbound is present over plaintext MQTT by default", async () => {
    const cfg = await loadMessagingConfig(
      tmpFile(
        "dual-plain.json",
        JSON.stringify({
          messaging: {
            local: { host: "localhost", port: 1883, clientId: "c-local" },
            northbound: { host: "northbound.mqtt.svc.cluster.local", port: 1884, clientId: "c-north" },
          },
        }),
      ),
    );
    expect(cfg.northbound).toBeDefined(); // the loader's topology decision: dual

    const provider = await StandaloneMqttProvider.connect(cfg);
    expect(hoisted.connectCalls).toHaveLength(2); // local + northbound
    expect(hoisted.connectCalls[0].url).toBe("mqtt://localhost:1883");
    expect(hoisted.connectCalls[1].url).toBe("mqtt://northbound.mqtt.svc.cluster.local:1884");

    await expect(
      provider.publishBytes("t", Buffer.from("x"), Destination.Northbound, Qos.ExactlyOnce),
    ).resolves.toBeUndefined();
    await provider.disconnect();
  });

  it("uses TLS for northbound when caPath is configured", async () => {
    const certPath = tmpFile("cert.pem", "CERTDATA");
    const keyPath = tmpFile("key.pem", "KEYDATA");
    const caPath = tmpFile("ca.pem", "CADATA");
    const cfg = await loadMessagingConfig(
      tmpFile(
        "dual.json",
        JSON.stringify({
          messaging: {
            local: { host: "localhost", port: 1883, clientId: "c-local" },
            northbound: {
              host: "cloud-mqtt.example.com",
              port: 8883,
              clientId: "c-north",
              credentials: { certPath, keyPath, caPath },
            },
          },
        }),
      ),
    );
    expect(cfg.northbound).toBeDefined(); // the loader's topology decision: dual

    const provider = await StandaloneMqttProvider.connect(cfg);
    expect(hoisted.connectCalls).toHaveLength(2); // local + northbound

    // call[0] = local broker (plaintext mqtt:// — no TLS material configured on the local leg).
    expect(hoisted.connectCalls[0].url).toBe("mqtt://localhost:1883");

    // call[1] = northbound broker over TLS (mqtts) with cert/key/ca material loaded.
    const northbound = hoisted.connectCalls[1];
    expect(northbound.url).toBe("mqtts://cloud-mqtt.example.com:8883");
    expect(Buffer.isBuffer(northbound.options.cert)).toBe(true);
    expect(Buffer.isBuffer(northbound.options.key)).toBe(true);
    expect(Buffer.isBuffer(northbound.options.ca)).toBe(true);
    expect((northbound.options.cert as Buffer).toString()).toBe("CERTDATA");
    // NO insecure fallback: rejectUnauthorized is never disabled (left to mqtt.js default = true);
    // SNI is derived from the URL host by mqtt.js (no override).
    expect(northbound.options.rejectUnauthorized).toBeUndefined();
    expect("servername" in northbound.options).toBe(false);

    // The northbound channel is reachable (no throw on a publish attempt).
    await expect(
      provider.publishBytes("t", Buffer.from("x"), Destination.Northbound, Qos.AtLeastOnce),
    ).resolves.toBeUndefined();
    await provider.disconnect();
  });
});

describe("FR-MSG-2: Kubernetes Service-DNS broker host", () => {
  it("uses a Service-DNS host verbatim in the broker URL (opaque string, no special handling)", async () => {
    const cfg = await loadMessagingConfig(
      tmpFile(
        "svcdns.json",
        JSON.stringify({
          messaging: {
            local: { host: "emqx.mqtt.svc.cluster.local", port: 1883, clientId: "c-k8s" },
          },
        }),
      ),
    );
    const provider = await StandaloneMqttProvider.connect(cfg);
    expect(hoisted.connectCalls).toHaveLength(1);
    expect(hoisted.connectCalls[0].url).toBe("mqtt://emqx.mqtt.svc.cluster.local:1883");
    await provider.disconnect();
  });

  it("a Service-DNS host on a TLS local broker uses mqtts with the DNS name (still no insecure fallback)", async () => {
    const caPath = tmpFile("ca.pem", "CADATA");
    const cfg = await loadMessagingConfig(
      tmpFile(
        "svcdns-tls.json",
        JSON.stringify({
          messaging: {
            local: {
              host: "emqx.mqtt.svc.cluster.local",
              port: 8883,
              clientId: "c-k8s-tls",
              credentials: { caPath },
            },
          },
        }),
      ),
    );
    const provider = await StandaloneMqttProvider.connect(cfg);
    expect(hoisted.connectCalls[0].url).toBe("mqtts://emqx.mqtt.svc.cluster.local:8883");
    expect(hoisted.connectCalls[0].options.rejectUnauthorized).toBeUndefined();
    await provider.disconnect();
  });

  it("a local broker with certPath but no caPath stays plaintext (issue #11 parity with Java/Python/Rust)", async () => {
    const certPath = tmpFile("client.pem", "CERT");
    const keyPath = tmpFile("client.key", "KEY");
    const cfg = await loadMessagingConfig(
      tmpFile(
        "local-cert-no-ca.json",
        JSON.stringify({
          messaging: {
            local: { host: "localhost", port: 1883, clientId: "c-cert-no-ca", credentials: { certPath, keyPath } },
          },
        }),
      ),
    );
    const provider = await StandaloneMqttProvider.connect(cfg);
    // `caPath` is the sole local-TLS trigger (matches Java/Python/Rust): no CA -> plaintext, and the
    // client cert is not loaded. Previously TS used mqtts here (caPath || certPath).
    expect(hoisted.connectCalls[0].url).toBe("mqtt://localhost:1883");
    expect(hoisted.connectCalls[0].options.cert).toBeUndefined();
    await provider.disconnect();
  });
});

describe("strict MQTT publish confirmation", () => {
  async function connectedProvider(): Promise<StandaloneMqttProvider> {
    const cfg = await loadMessagingConfig(
      tmpFile(
        "confirmed.json",
        JSON.stringify({ messaging: { local: { host: "localhost", port: 1883, clientId: "confirmed" } } }),
      ),
    );
    return StandaloneMqttProvider.connect(cfg);
  }

  it("does not resolve until mqtt.js reports the QoS-1 PUBACK callback and preserves exact bytes", async () => {
    hoisted.deferPublishAck = true;
    const provider = await connectedProvider();
    const exact = Buffer.from([0, 1, 2, 255]);
    let settled = false;
    const pending = provider
      .publishBytesConfirmed("camera/reply", exact, Destination.Local, Qos.AtLeastOnce, 1_000)
      .then(() => {
        settled = true;
      });
    await new Promise<void>((resolve) => setImmediate(resolve));

    expect(settled).toBe(false);
    expect(hoisted.publishCalls).toHaveLength(1);
    expect(hoisted.publishCalls[0]).toMatchObject({ topic: "camera/reply", options: { qos: 1 } });
    expect(hoisted.publishCalls[0].payload.equals(exact)).toBe(true);
    hoisted.publishCallbacks.shift()?.();
    await pending;
    expect(settled).toBe(true);
    await provider.disconnect();
  });

  it("rejects disconnect-before-PUBACK as ambiguous even if a late callback arrives", async () => {
    hoisted.deferPublishAck = true;
    const provider = await connectedProvider();
    const pending = provider.publishBytesConfirmed(
      "camera/reply",
      Buffer.from("payload"),
      Destination.Local,
      Qos.AtLeastOnce,
      1_000,
    );
    await new Promise<void>((resolve) => setImmediate(resolve));
    hoisted.clients[0].emit("close");

    await expect(pending).rejects.toMatchObject({ reason: "transport" });
    expect(() => hoisted.publishCallbacks.shift()?.()).not.toThrow();
    await provider.disconnect();
  });

  it("rejects timeout, disconnected start, non-QoS1, and immediate publish errors", async () => {
    hoisted.deferPublishAck = true;
    const provider = await connectedProvider();
    await expect(
      provider.publishBytesConfirmed("t", Buffer.from("x"), Destination.Local, Qos.AtLeastOnce, 20),
    ).rejects.toMatchObject({ reason: "timeout" });
    // Exercise the late-PUBACK callback after timeout: it must be harmless and must not be
    // mistaken for the callback belonging to the next operation.
    expect(() => hoisted.publishCallbacks.shift()?.()).not.toThrow();

    hoisted.clients[0].connected = false;
    await expect(
      provider.publishBytesConfirmed("t", Buffer.from("x"), Destination.Local, Qos.AtLeastOnce, 20),
    ).rejects.toMatchObject({ reason: "transport" });
    await expect(
      provider.publishBytesConfirmed("t", Buffer.from("x"), Destination.Local, Qos.AtMostOnce, 20),
    ).rejects.toThrow(/QoS 1/);

    hoisted.clients[0].connected = true;
    const failed = provider.publishBytesConfirmed(
      "t",
      Buffer.from("x"),
      Destination.Local,
      Qos.AtLeastOnce,
      1_000,
    );
    hoisted.publishCallbacks.shift()?.(new Error("broker rejected publish"));
    await expect(failed).rejects.toMatchObject({ reason: "transport" });
    await provider.disconnect();
  });

  it("rejects immediately when the bounded confirmed-publish capacity is exhausted", async () => {
    const provider = await connectedProvider();
    (provider as unknown as { confirmedInFlight: number }).confirmedInFlight = 1024;
    await expect(
      provider.publishBytesConfirmed("t", Buffer.from("x"), Destination.Local, Qos.AtLeastOnce, 100),
    ).rejects.toMatchObject({ reason: "backpressure" });
    await provider.disconnect();
  });
});
