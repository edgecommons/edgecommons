/**
 * Messaging provider — STANDALONE dual-broker MQTT (over `mqtt.js`).
 *
 * The STANDALONE-mode {@link MessagingProvider}: a local broker (always) plus an
 * optional AWS IoT Core broker (mutual-TLS). Connects block until confirmed;
 * subscribe blocks until SUBACK. Inbound routing matches MQTT wildcards per
 * subscription (mqtt.js delivers every message on one `message` event).
 */
import { readFileSync } from "fs";

import mqtt, { MqttClient } from "mqtt";

import { GgError } from "../errors";
import { logger } from "../logging";
import { BrokerConfig, MessagingConfig, resolvedHost } from "./config";
import { Destination, MessagingProvider, Qos, RawSubscription } from "./types";

interface Sub {
  filter: string;
  qos: Qos;
  onMessage: (topic: string, payload: Buffer) => void;
}

function qosNum(qos: Qos): 0 | 1 {
  return qos === Qos.AtMostOnce ? 0 : 1;
}

/** Whether an MQTT topic `filter` (with `+`/`#`) matches a concrete `topic`. */
export function topicMatches(filter: string, topic: string): boolean {
  const f = filter.split("/");
  const t = topic.split("/");
  for (let i = 0; i < f.length; i++) {
    if (f[i] === "#") return true;
    if (i >= t.length) return false;
    if (f[i] !== "+" && f[i] !== t[i]) return false;
  }
  return f.length === t.length;
}

/** One MQTT broker connection plus its subscription routing table. */
class BrokerChannel {
  readonly subs: Sub[] = [];
  constructor(readonly client: MqttClient) {
    client.on("message", (topic: string, payload: Buffer) => {
      for (const sub of this.subs) {
        if (topicMatches(sub.filter, topic)) {
          // Contain a throwing subscriber: mqtt.js delivers all matching subs on
          // one `message` event, so an uncaught throw would abort the loop and
          // starve the remaining subscribers (parity with the GG IPC fix).
          try {
            sub.onMessage(topic, payload);
          } catch (e) {
            logger.warn(`ggcommons: MQTT message handler threw for ${topic}: ${String(e)}`);
          }
        }
      }
    });
    // On every (re)connect, re-issue known subscriptions so a broker drop + auto-reconnect does
    // not silently stop delivering (parity with Java/Python/Rust). The first connect's subs are
    // issued by subscribeRaw; this fires meaningfully only on reconnects, when subs is populated.
    client.on("connect", () => {
      for (const sub of this.subs) {
        client.subscribe(sub.filter, { qos: qosNum(sub.qos) }, (err) => {
          if (err) logger.warn(`ggcommons: resubscribe to ${sub.filter} failed: ${String(err)}`);
        });
      }
    });
  }
}

/** STANDALONE-mode dual-broker MQTT provider. */
export class StandaloneMqttProvider implements MessagingProvider {
  private constructor(
    private readonly local: BrokerChannel,
    private readonly iot?: BrokerChannel,
  ) {}

  /** Connect the local broker (and IoT Core if configured), resolving when ready. */
  static async connect(config: MessagingConfig): Promise<StandaloneMqttProvider> {
    // Local broker uses TLS when CA/cert material is configured (CA-only server TLS or mTLS),
    // not just for IoT Core — previously the local broker was always plaintext.
    const lc = config.local.credentials;
    const localTls = !!(lc?.caPath || lc?.certPath);
    const local = new BrokerChannel(await connectBroker(config.local, localTls));
    const iot = config.iotCore ? new BrokerChannel(await connectBroker(config.iotCore, true)) : undefined;
    return new StandaloneMqttProvider(local, iot);
  }

  private channel(dest: Destination): BrokerChannel {
    if (dest === Destination.IotCore) {
      if (!this.iot) throw GgError.messaging("no IoT Core broker configured (messaging.iotCore)");
      return this.iot;
    }
    return this.local;
  }

  publishBytes(topic: string, payload: Buffer, dest: Destination, qos: Qos): Promise<void> {
    const ch = this.channel(dest);
    return new Promise((resolve, reject) => {
      ch.client.publish(topic, payload, { qos: qosNum(qos) }, (err) =>
        err ? reject(GgError.messaging(`publish to ${topic} failed: ${err}`)) : resolve(),
      );
    });
  }

  subscribeRaw(
    filter: string,
    dest: Destination,
    qos: Qos,
    onMessage: (topic: string, payload: Buffer) => void,
  ): Promise<RawSubscription> {
    const ch = this.channel(dest);
    return new Promise((resolve, reject) => {
      ch.client.subscribe(filter, { qos: qosNum(qos) }, (err) => {
        if (err) {
          reject(GgError.messaging(`subscribe to ${filter} failed: ${err}`));
          return;
        }
        const sub: Sub = { filter, qos, onMessage };
        ch.subs.push(sub);
        resolve({
          unsubscribe: () =>
            new Promise<void>((res) => {
              const idx = ch.subs.indexOf(sub);
              if (idx >= 0) ch.subs.splice(idx, 1);
              ch.client.unsubscribe(filter, () => res());
            }),
        });
      });
    });
  }

  async disconnect(): Promise<void> {
    await Promise.all(
      [this.local, this.iot]
        .filter((c): c is BrokerChannel => c !== undefined)
        .map((c) => new Promise<void>((res) => c.client.end(false, {}, () => res()))),
    );
  }
}

/** Connect one broker, resolving on CONNACK and rejecting on the first error. */
function connectBroker(broker: BrokerConfig, tls: boolean): Promise<MqttClient> {
  const host = resolvedHost(broker);
  const url = `${tls ? "mqtts" : "mqtt"}://${host}:${broker.port}`;
  const options: mqtt.IClientOptions = {
    clientId: broker.clientId,
    // Auto-reconnect after a broker drop (BrokerChannel re-issues subscriptions on reconnect);
    // previously 0 = disabled, leaving the component silently disconnected after any outage.
    reconnectPeriod: 5_000,
    connectTimeout: 15_000,
  };
  const creds = broker.credentials;
  if (creds?.username) options.username = creds.username;
  if (creds?.password) options.password = creds.password;
  if (tls && creds) {
    if (creds.certPath) options.cert = readFileSync(creds.certPath);
    if (creds.keyPath) options.key = readFileSync(creds.keyPath);
    if (creds.caPath) options.ca = readFileSync(creds.caPath);
  }
  return new Promise((resolve, reject) => {
    const client = mqtt.connect(url, options);
    client.once("connect", () => resolve(client));
    client.once("error", (err) => {
      // Initial connect failed: stop the client so auto-reconnect doesn't leak a background
      // retry loop after we reject (reconnects after a successful connect are handled silently).
      client.end(true, {}, () => {});
      reject(GgError.messaging(`connect to ${url} failed: ${err}`));
    });
  });
}
