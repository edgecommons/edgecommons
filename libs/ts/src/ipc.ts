/**
 * Messaging — Greengrass IPC provider (TypeScript, GREENGRASS mode).
 *
 * The GREENGRASS-mode counterpart of {@link StandaloneProvider}: it moves the same
 * cross-language {@link Message} envelopes over Greengrass **local pub/sub** (and
 * the IoT Core MQTT bridge) using `aws-iot-device-sdk-v2`'s `greengrasscoreipc`
 * client. Its public surface matches `StandaloneProvider` so the two transports are
 * interchangeable.
 *
 * Wire parity with the Java/Python/Rust IPC providers:
 * - an **envelope** is published as a `binaryMessage` carrying the envelope JSON,
 * - a **raw** payload is published as a `jsonMessage` (matching Python),
 * - inbound, both shapes are decoded and classified via {@link Message.fromObject}
 *   / {@link Message.fromWire}, and the delivered topic is the message's own
 *   `context.topic` (so wildcard subscriptions and reply routing work),
 * - request/reply uses the shared `ggcommons/reply-…` ephemeral-topic convention
 *   with a copied `correlation_id`.
 *
 * Note: the JS SDK exposes the **V1** IPC client surface (manual streaming
 * operations); the simplified clientV2 used by Python/Java is Java/Python-only.
 * This requires a running Greengrass nucleus (env `SVCUID`, the domain-socket path)
 * and an `accessControl` policy in the component recipe for the topics used.
 */
import { randomUUID } from "crypto";

import { greengrasscoreipc, eventstream_rpc } from "aws-iot-device-sdk-v2";

import model = greengrasscoreipc.model;

import { Message } from "./message";
import { MessageHandler, REPLY_TOPIC_PREFIX } from "./standalone";

/** QoS for the IoT Core bridge (mirrors the other libs' default). */
export enum IpcQos {
  AtMostOnce = "0",
  AtLeastOnce = "1",
}

/** Connection options for {@link IpcProvider.connect}. */
export interface IpcOptions {
  /** Receive a component's own published messages too (default: false). */
  receiveOwnMessages?: boolean;
}

type LocalStream = eventstream_rpc.StreamingOperation<
  model.SubscribeToTopicRequest,
  model.SubscribeToTopicResponse,
  void,
  model.SubscriptionResponseMessage
>;
type IotStream = eventstream_rpc.StreamingOperation<
  model.SubscribeToIoTCoreRequest,
  model.SubscribeToIoTCoreResponse,
  void,
  model.IoTCoreMessage
>;

/**
 * Greengrass IPC messaging provider. Construct via {@link IpcProvider.connect},
 * which resolves only once the IPC connection to the nucleus is established.
 */
export class IpcProvider {
  private readonly client: greengrasscoreipc.Client;
  private readonly receiveMode: model.ReceiveMode;
  /** Live local subscriptions keyed by topic filter (for unsubscribe/close). */
  private readonly localStreams = new Map<string, LocalStream>();
  /** Live IoT Core subscriptions keyed by topic filter. */
  private readonly iotStreams = new Map<string, IotStream>();

  private constructor(client: greengrasscoreipc.Client, receiveMode: model.ReceiveMode) {
    this.client = client;
    this.receiveMode = receiveMode;
  }

  /** Connect to the Greengrass nucleus, resolving once IPC is established. */
  static async connect(opts: IpcOptions = {}): Promise<IpcProvider> {
    const client = greengrasscoreipc.createClient();
    await client.connect();
    const receiveMode = opts.receiveOwnMessages
      ? model.ReceiveMode.RECEIVE_ALL_MESSAGES
      : model.ReceiveMode.RECEIVE_MESSAGES_FROM_OTHERS;
    return new IpcProvider(client, receiveMode);
  }

  /** Publish a message envelope to a local pub/sub `topic`. */
  async publish(topic: string, message: Message): Promise<void> {
    await this.client.publishToTopic({
      topic,
      publishMessage: { binaryMessage: { message: message.toJSON() } },
    });
  }

  /** Publish a raw (non-envelope) JSON payload to a local pub/sub `topic`. */
  async publishRaw(topic: string, payload: unknown): Promise<void> {
    await this.client.publishToTopic({
      topic,
      publishMessage: { jsonMessage: { message: payload } },
    });
  }

  /** Publish a message envelope to AWS IoT Core via the IPC bridge. */
  async publishToIotCore(
    topic: string,
    message: Message,
    qos: IpcQos = IpcQos.AtLeastOnce,
  ): Promise<void> {
    await this.client.publishToIoTCore({
      topicName: topic,
      qos: qos as unknown as model.QOS,
      payload: message.toJSON(),
    });
  }

  /**
   * Subscribe to a local pub/sub `filter`, resolving once the subscription is
   * confirmed by the nucleus (the streaming operation is activated).
   */
  async subscribe(filter: string, handler: MessageHandler): Promise<void> {
    const op = this.client.subscribeToTopic({ topic: filter, receiveMode: this.receiveMode });
    op.on("message", (event: model.SubscriptionResponseMessage) => {
      const delivered = decodeLocal(event);
      if (delivered) {
        handler(delivered.topic, delivered.message);
      }
    });
    op.on("streamError", () => true); // keep the stream open on error
    await op.activate();
    this.localStreams.set(filter, op);
  }

  /** Subscribe to an AWS IoT Core `filter` via the IPC bridge. */
  async subscribeToIotCore(
    filter: string,
    handler: MessageHandler,
    qos: IpcQos = IpcQos.AtLeastOnce,
  ): Promise<void> {
    const op = this.client.subscribeToIoTCore({
      topicName: filter,
      qos: qos as unknown as model.QOS,
    });
    op.on("message", (event: model.IoTCoreMessage) => {
      const payload = event.message?.payload;
      const topic = event.message?.topicName ?? filter;
      if (payload !== undefined) {
        handler(topic, Message.fromWire(toBuffer(payload)));
      }
    });
    op.on("streamError", () => true);
    await op.activate();
    this.iotStreams.set(filter, op);
  }

  /** Unsubscribe from a local `filter` (closes the streaming operation). */
  async unsubscribe(filter: string): Promise<void> {
    const op = this.localStreams.get(filter);
    if (op) {
      this.localStreams.delete(filter);
      await op.close();
    }
  }

  /** Unsubscribe from an IoT Core `filter`. */
  async unsubscribeFromIotCore(filter: string): Promise<void> {
    const op = this.iotStreams.get(filter);
    if (op) {
      this.iotStreams.delete(filter);
      await op.close();
    }
  }

  /**
   * Send a request over local pub/sub and resolve with the correlated reply.
   * Mirrors {@link StandaloneProvider.request}: subscribe an ephemeral reply
   * topic, set it as `reply_to`, publish, resolve on the first reply (then
   * unsubscribe). Rejects on timeout (default 8s).
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

  /** Close every subscription and the IPC connection. */
  async disconnect(): Promise<void> {
    const ops = [...this.localStreams.values(), ...this.iotStreams.values()];
    this.localStreams.clear();
    this.iotStreams.clear();
    await Promise.allSettled(ops.map((op) => op.close()));
    await this.client.close();
  }
}

/** One decoded local message: the delivered topic and the classified Message. */
interface Delivered {
  topic: string;
  message: Message;
}

/**
 * Decode an inbound local pub/sub event into a topic + {@link Message}. A
 * `binaryMessage` carries the envelope JSON bytes; a `jsonMessage` carries an
 * already-parsed object. The topic is the message's own `context.topic` when
 * present (matching the other libs), else undefined → dropped.
 */
function decodeLocal(event: model.SubscriptionResponseMessage): Delivered | null {
  if (event.binaryMessage) {
    const topic = event.binaryMessage.context?.topic;
    const payload = event.binaryMessage.message;
    if (topic === undefined || payload === undefined) return null;
    return { topic, message: Message.fromWire(toBuffer(payload)) };
  }
  if (event.jsonMessage) {
    const topic = event.jsonMessage.context?.topic;
    if (topic === undefined) return null;
    return { topic, message: Message.fromObject(event.jsonMessage.message) };
  }
  return null;
}

/** Normalize an eventstream payload (string | ArrayBuffer | view) to a Buffer. */
function toBuffer(payload: string | ArrayBuffer | ArrayBufferView): Buffer {
  if (typeof payload === "string") return Buffer.from(payload, "utf8");
  if (payload instanceof ArrayBuffer) return Buffer.from(payload);
  return Buffer.from(payload.buffer, payload.byteOffset, payload.byteLength);
}
