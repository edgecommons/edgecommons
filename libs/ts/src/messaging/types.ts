/**
 * Messaging — shared types: transport {@link MessagingProvider}, the user-facing
 * {@link IMessagingService}, and the {@link Destination}/{@link Qos} enums.
 *
 * Mirrors the Rust `MessagingProvider` (raw bytes transport) / `MessagingService`
 * (message-level publish/subscribe + request/reply) split, and the Java/Python
 * `IMessagingService` contract — explicit local / IoT Core method pairs.
 */
import type { Message } from "../message";

/** Where a message goes / comes from. */
export enum Destination {
  /** Local broker (STANDALONE MQTT) or Greengrass IPC local pub/sub. */
  Local = "local",
  /** AWS IoT Core. */
  IotCore = "iotcore",
}

/** MQTT quality of service. */
export enum Qos {
  AtMostOnce = "atMostOnce",
  AtLeastOnce = "atLeastOnce",
}

/** A handler invoked for each message delivered to a subscription. */
export type MessageHandler = (topic: string, message: Message) => void | Promise<void>;

/** A live raw-transport subscription; closing it unsubscribes at the broker. */
export interface RawSubscription {
  unsubscribe(): Promise<void>;
}

/**
 * Transport layer: moves raw bytes over a broker/IPC. The {@link DefaultMessagingService}
 * adds message (de)serialization, dispatch, and request/reply on top. Mirrors the
 * Rust `MessagingProvider` trait.
 */
export interface MessagingProvider {
  /** Publish raw bytes to `topic` on `dest` at `qos`. */
  publishBytes(topic: string, payload: Buffer, dest: Destination, qos: Qos): Promise<void>;
  /** Subscribe to `filter` on `dest`; deliver each `(topic, payload)` to `onMessage`. */
  subscribeRaw(
    filter: string,
    dest: Destination,
    qos: Qos,
    onMessage: (topic: string, payload: Buffer) => void,
  ): Promise<RawSubscription>;
  /**
   * Whether the transport is currently connected (the readiness signal consumed by the `/readyz`
   * health probe; FR-HB-1). MUST be cheap and non-blocking — it is polled on every probe and MUST NOT
   * be consulted by `/livez`.
   */
  connected(): boolean;
  /** Close the transport and all subscriptions. */
  disconnect(): Promise<void>;
}

/**
 * A pending request's reply — awaitable (like a Promise) and cancelable. The Rust
 * `ReplyFuture` / Java `CompletableFuture<Message>` / Python `Iou` analog.
 */
export class ReplyFuture implements PromiseLike<Message> {
  constructor(
    private readonly promise: Promise<Message>,
    private readonly canceler: () => void,
  ) {}

  then<TResult1 = Message, TResult2 = never>(
    onfulfilled?: ((value: Message) => TResult1 | PromiseLike<TResult1>) | null,
    onrejected?: ((reason: unknown) => TResult2 | PromiseLike<TResult2>) | null,
  ): PromiseLike<TResult1 | TResult2> {
    return this.promise.then(onfulfilled, onrejected);
  }

  /** Abandon the pending request, cleaning up its reply subscription. */
  cancel(): void {
    this.canceler();
  }
}

/**
 * Transport-agnostic messaging operations over {@link Message}s, with explicit
 * local / IoT Core method pairs (mirrors the Java/Python `IMessagingService` and
 * Rust `MessagingService`).
 */
export interface IMessagingService {
  publish(topic: string, msg: Message): Promise<void>;
  publishToIotCore(topic: string, msg: Message, qos?: Qos): Promise<void>;
  publishRaw(topic: string, payload: unknown): Promise<void>;
  publishToIotCoreRaw(topic: string, payload: unknown, qos?: Qos): Promise<void>;

  /**
   * Register a callback for `filter` on the local broker. `maxMessages` bounds the
   * client-side queue; `maxConcurrency` bounds simultaneous handler invocations
   * (`1` = serial, ordered).
   */
  subscribe(filter: string, handler: MessageHandler, maxMessages?: number, maxConcurrency?: number): Promise<void>;
  subscribeToIotCore(
    filter: string,
    handler: MessageHandler,
    qos?: Qos,
    maxMessages?: number,
    maxConcurrency?: number,
  ): Promise<void>;

  unsubscribe(filter: string): Promise<void>;
  unsubscribeFromIotCore(filter: string): Promise<void>;

  /** Send a request on the local broker; await/timeout the returned {@link ReplyFuture}. */
  request(topic: string, msg: Message, timeoutMs?: number): ReplyFuture;
  requestFromIotCore(topic: string, msg: Message, timeoutMs?: number): ReplyFuture;

  reply(request: Message, reply: Message): Promise<void>;
  replyToIotCore(request: Message, reply: Message): Promise<void>;

  cancelRequest(reply: ReplyFuture): void;
  cancelRequestFromIotCore(reply: ReplyFuture): void;

  /**
   * Whether the underlying messaging transport is currently connected. The readiness signal behind the
   * `/readyz` health probe (FR-HB-1): the runtime is "ready" only when this is `true` (and the app's
   * `setReady` flag is set and the process is not shutting down). Cheap, non-blocking, and never
   * consulted by `/livez` (a broker outage must not fail liveness).
   */
  connected(): boolean;
}

/** Prefix for generated reply topics. Matches the other libraries exactly. */
export const REPLY_TOPIC_PREFIX = "ggcommons/reply-";
