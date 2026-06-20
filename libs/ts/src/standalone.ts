/**
 * Messaging — STANDALONE provider (TypeScript).
 *
 * A minimal STANDALONE-mode messaging provider over `mqtt.js`, mirroring the
 * Java/Python/Rust libraries' contract for the local broker: connect (block until
 * connected), subscribe (block until SUBACK), publish / publishRaw, and the
 * request/reply pattern (ephemeral reply topic + copied `correlation_id`).
 *
 * This spike implements the **local** broker only (the dual-MQTT IoT-Core leg of
 * the full libraries is out of scope here). Topic routing matches MQTT wildcards
 * (`+`, `#`) against each subscription, since `mqtt.js` delivers every message on
 * one `message` event.
 */
import mqtt, { MqttClient } from "mqtt";

import { Message } from "./message";

/** Prefix for generated reply topics. Matches the other libraries exactly. */
export const REPLY_TOPIC_PREFIX = "ggcommons/reply-";

/** A handler invoked for each message delivered to a subscription. */
export type MessageHandler = (topic: string, message: Message) => void;

/** Connection options for {@link StandaloneProvider.connect}. */
export interface StandaloneOptions {
  host?: string;
  port?: number;
  clientId?: string;
}

import { randomUUID } from "crypto";

interface Subscription {
  filter: string;
  handler: MessageHandler;
}

/**
 * STANDALONE-mode messaging provider over a single local MQTT broker.
 *
 * Construct via {@link StandaloneProvider.connect}, which resolves only once the
 * MQTT CONNACK has arrived, mirroring the other libraries' "block until
 * connected" semantics.
 */
export class StandaloneProvider {
  private readonly client: MqttClient;
  private readonly subscriptions: Subscription[] = [];

  private constructor(client: MqttClient) {
    this.client = client;
    this.client.on("message", (topic: string, payload: Buffer) => {
      const message = Message.fromWire(payload);
      for (const sub of this.subscriptions) {
        if (topicMatches(sub.filter, topic)) {
          sub.handler(topic, message);
        }
      }
    });
  }

  /** Connect to the local broker, resolving once the connection is confirmed. */
  static connect(opts: StandaloneOptions = {}): Promise<StandaloneProvider> {
    const host = opts.host ?? "localhost";
    const port = opts.port ?? 1883;
    const clientId = opts.clientId ?? `ggcommons-ts-${randomUUID()}`;
    return new Promise((resolve, reject) => {
      const client = mqtt.connect(`mqtt://${host}:${port}`, {
        clientId,
        reconnectPeriod: 0,
        connectTimeout: 10_000,
      });
      client.once("connect", () => resolve(new StandaloneProvider(client)));
      client.once("error", (err: Error) => reject(err));
    });
  }

  /** Subscribe to `filter`, resolving once the broker confirms the SUBACK. */
  subscribe(filter: string, handler: MessageHandler): Promise<void> {
    return new Promise((resolve, reject) => {
      this.client.subscribe(filter, { qos: 1 }, (err) => {
        if (err) {
          reject(err);
          return;
        }
        this.subscriptions.push({ filter, handler });
        resolve();
      });
    });
  }

  /** Publish a message envelope to `topic`. */
  publish(topic: string, message: Message): Promise<void> {
    return this.publishBytes(topic, message.toJSON());
  }

  /** Publish a raw (non-envelope) JSON payload to `topic`. */
  publishRaw(topic: string, payload: unknown): Promise<void> {
    return this.publishBytes(topic, JSON.stringify(payload));
  }

  private publishBytes(topic: string, payload: string): Promise<void> {
    return new Promise((resolve, reject) => {
      this.client.publish(topic, payload, { qos: 1 }, (err) =>
        err ? reject(err) : resolve(),
      );
    });
  }

  /**
   * Send a request and resolve with the correlated reply. Subscribes to a unique
   * ephemeral reply topic, sets it as the request's `reply_to`, publishes, and
   * resolves on the first message received there (then unsubscribes). Rejects on
   * timeout (default 8s).
   */
  request(topic: string, request: Message, timeoutMs = 8000): Promise<Message> {
    const replyTopic = `${REPLY_TOPIC_PREFIX}${randomUUID()}`;
    request.header.reply_to = replyTopic;

    return new Promise<Message>((resolve, reject) => {
      let settled = false;
      const finish = (fn: () => void) => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        this.unsubscribe(replyTopic).finally(fn);
      };

      const timer = setTimeout(
        () => finish(() => reject(new Error("request timed out"))),
        timeoutMs,
      );

      this.subscribe(replyTopic, (_t, reply) => finish(() => resolve(reply)))
        .then(() => this.publish(topic, request))
        .catch((err) => finish(() => reject(err)));
    });
  }

  /**
   * Reply to a received request: copy its `correlation_id` onto `reply` and
   * publish to the request's `reply_to` topic.
   */
  reply(request: Message, reply: Message): Promise<void> {
    const replyTo = request.getReplyTo();
    if (!replyTo) {
      return Promise.reject(new Error("cannot reply: request has no reply_to"));
    }
    reply.header.correlation_id = request.getCorrelationId();
    return this.publish(replyTo, reply);
  }

  /** Unsubscribe from `filter` and drop its handler(s). */
  unsubscribe(filter: string): Promise<void> {
    for (let i = this.subscriptions.length - 1; i >= 0; i--) {
      if (this.subscriptions[i].filter === filter) {
        this.subscriptions.splice(i, 1);
      }
    }
    return new Promise((resolve) => {
      this.client.unsubscribe(filter, () => resolve());
    });
  }

  /** Close the connection. */
  disconnect(): Promise<void> {
    return new Promise((resolve) => {
      this.client.end(false, {}, () => resolve());
    });
  }
}

/**
 * Whether an MQTT topic `filter` (with `+`/`#` wildcards) matches a concrete
 * `topic`. Implements the standard MQTT topic-matching rules.
 */
export function topicMatches(filter: string, topic: string): boolean {
  const f = filter.split("/");
  const t = topic.split("/");
  for (let i = 0; i < f.length; i++) {
    if (f[i] === "#") {
      return true;
    }
    if (i >= t.length) {
      return false;
    }
    if (f[i] !== "+" && f[i] !== t[i]) {
      return false;
    }
  }
  return f.length === t.length;
}
