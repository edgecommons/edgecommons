/**
 * Phase 1b messaging-topology tests (FR-MSG-2 / FR-MSG-3).
 *
 * These exercise {@link StandaloneMqttProvider.connect} with the `mqtt` module mocked, so the
 * single- vs dual-broker wiring and the per-broker TLS/URL decisions are verified deterministically
 * without a live broker:
 *
 *  - FR-MSG-2: a Kubernetes Service-DNS host (`emqx.mqtt.svc.cluster.local`) is an opaque string and
 *    is used verbatim in the broker URL (no special handling, no insecure behavior).
 *  - FR-MSG-3: `messaging.iotCore` presence selects the topology — single broker (air-gapped, local
 *    only) when absent, dual (local + AWS IoT Core) when present. The IoT Core leg keeps mutual TLS
 *    (mqtts + cert/key/ca) with NO insecure fallback (no `rejectUnauthorized: false`); SNI is left to
 *    mqtt.js, which derives the servername from the URL host.
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
}));

vi.mock("mqtt", () => {
  const connect = (url: string, options: Record<string, unknown>): EventEmitter => {
    hoisted.connectCalls.push({ url, options });
    const client = new EventEmitter() as EventEmitter & Record<string, unknown>;
    client.publish = (_t: string, _p: unknown, _o: unknown, cb?: (e?: Error) => void): void => cb?.();
    client.subscribe = (_f: string, _o: unknown, cb?: (e?: Error) => void): void => cb?.();
    client.unsubscribe = (_f: string, cb?: () => void): void => cb?.();
    client.end = (_force?: unknown, _o?: unknown, cb?: () => void): void => cb?.();
    // Emit CONNACK asynchronously so connectBroker's once("connect") handler is registered first.
    setImmediate(() => client.emit("connect"));
    return client;
  };
  return { default: { connect }, connect };
});

import { StandaloneMqttProvider } from "../src/messaging/standalone-provider";
import { loadMessagingConfig } from "../src/messaging/config";
import { Destination, Qos } from "../src/messaging/types";
import { GgError } from "../src/errors";

const tmp: string[] = [];
function tmpFile(name: string, contents: string): string {
  const p = path.join(os.tmpdir(), `ggc-topo-${Math.random().toString(36).slice(2)}-${name}`);
  fs.writeFileSync(p, contents);
  tmp.push(p);
  return p;
}

beforeEach(() => {
  hoisted.connectCalls.length = 0;
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
  it("connects only the local broker when messaging.iotCore is absent", async () => {
    const cfg = await loadMessagingConfig(
      tmpFile(
        "single.json",
        JSON.stringify({ messaging: { local: { host: "localhost", port: 1883, clientId: "c-local" } } }),
      ),
    );
    expect(cfg.iotCore).toBeUndefined(); // the loader's topology decision: single

    const provider = await StandaloneMqttProvider.connect(cfg);
    expect(hoisted.connectCalls).toHaveLength(1); // exactly one broker connection
    expect(hoisted.connectCalls[0].url).toBe("mqtt://localhost:1883");

    // No IoT Core channel exists: publishing to IoT Core throws synchronously.
    expect(() =>
      provider.publishBytes("t", Buffer.from("x"), Destination.IoTCore, Qos.AtLeastOnce),
    ).toThrow(GgError);
    await provider.disconnect();
  });
});

describe("FR-MSG-3: dual-MQTT topology + IoT Core mutual TLS", () => {
  it("connects both brokers when messaging.iotCore is present, IoT Core over mutual TLS with no insecure fallback", async () => {
    const certPath = tmpFile("cert.pem", "CERTDATA");
    const keyPath = tmpFile("key.pem", "KEYDATA");
    const caPath = tmpFile("ca.pem", "CADATA");
    const cfg = await loadMessagingConfig(
      tmpFile(
        "dual.json",
        JSON.stringify({
          messaging: {
            local: { host: "localhost", port: 1883, clientId: "c-local" },
            iotCore: {
              endpoint: "abc123.iot.us-east-1.amazonaws.com",
              port: 8883,
              clientId: "c-iot",
              credentials: { certPath, keyPath, caPath },
            },
          },
        }),
      ),
    );
    expect(cfg.iotCore).toBeDefined(); // the loader's topology decision: dual

    const provider = await StandaloneMqttProvider.connect(cfg);
    expect(hoisted.connectCalls).toHaveLength(2); // local + IoT Core

    // call[0] = local broker (plaintext mqtt:// — no TLS material configured on the local leg).
    expect(hoisted.connectCalls[0].url).toBe("mqtt://localhost:1883");

    // call[1] = IoT Core broker over mutual TLS (mqtts) with cert/key/ca material loaded.
    const iot = hoisted.connectCalls[1];
    expect(iot.url).toBe("mqtts://abc123.iot.us-east-1.amazonaws.com:8883");
    expect(Buffer.isBuffer(iot.options.cert)).toBe(true);
    expect(Buffer.isBuffer(iot.options.key)).toBe(true);
    expect(Buffer.isBuffer(iot.options.ca)).toBe(true);
    expect((iot.options.cert as Buffer).toString()).toBe("CERTDATA");
    // NO insecure fallback: rejectUnauthorized is never disabled (left to mqtt.js default = true);
    // SNI is derived from the URL host by mqtt.js (no override).
    expect(iot.options.rejectUnauthorized).toBeUndefined();
    expect("servername" in iot.options).toBe(false);

    // The IoT Core channel is reachable (no throw on a publish attempt).
    await expect(
      provider.publishBytes("t", Buffer.from("x"), Destination.IoTCore, Qos.AtLeastOnce),
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
