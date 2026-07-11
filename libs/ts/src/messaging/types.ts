/**
 * Messaging — shared types: transport {@link MessagingProvider}, the user-facing
 * {@link IMessagingService}, and the {@link Destination}/{@link Qos} enums.
 *
 * Mirrors the Rust `MessagingProvider` (raw bytes transport) / `MessagingService`
 * (message-level publish/subscribe + request/reply) split, and the Java/Python
 * `IMessagingService` contract — explicit local / northbound method pairs.
 */
import type { Message } from "../message";

/** Where a message goes / comes from. */
export enum Destination {
  /** Local broker (STANDALONE MQTT) or Greengrass IPC local pub/sub. */
  Local = "local",
  /** The northbound transport. */
  Northbound = "northbound",
}

/** MQTT quality of service. */
export enum Qos {
  AtMostOnce = "atMostOnce",
  AtLeastOnce = "atLeastOnce",
  /** MQTT QoS 2. Supported only by the standalone local MQTT provider. */
  ExactlyOnce = "exactlyOnce",
}

/** Hard bound on transport operations concurrently waiting for delivery acknowledgement. */
export const MAX_IN_FLIGHT_CONFIRMED_PUBLISHES = 1024;

/** Why strict publication could not prove positive transport acknowledgement. */
export type PublishConfirmationReason =
  | "timeout"
  | "transport"
  | "backpressure"
  | "unsupported"
  | "invalidEnvelope";

/**
 * Strict confirmed-publish failure. The caller must treat delivery as unsuccessful or ambiguous
 * and may retry the exact same encoded envelope.
 */
export class PublishConfirmationError extends Error {
  constructor(
    readonly reason: PublishConfirmationReason,
    message: string,
    options?: { cause?: unknown },
  ) {
    super(message);
    this.name = "PublishConfirmationError";
    if (options && "cause" in options) {
      (this as Error & { cause?: unknown }).cause = options.cause;
    }
  }
}

/** Validate the shared QoS-1 + bounded-timeout contract. */
export function validateConfirmedPublish(qos: Qos, timeoutMs: number): void {
  if (qos !== Qos.AtLeastOnce) {
    throw new Error("confirmed publish requires explicit QoS 1 (AtLeastOnce)");
  }
  if (!Number.isFinite(timeoutMs) || !Number.isInteger(timeoutMs) || timeoutMs <= 0) {
    throw new Error("confirmed publish timeoutMs must be a positive integer");
  }
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
  /**
   * Strict raw transport publication. MQTT resolves only after PUBACK; Greengrass resolves only
   * after successful IPC operation completion. Optional for source compatibility with custom
   * providers; callers must throw `unsupported` when absent rather than using `publishBytes`.
   */
  publishBytesConfirmed?(
    topic: string,
    payload: Buffer,
    dest: Destination,
    qos: Qos,
    timeoutMs: number,
  ): Promise<void>;
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
 * The `request()` deadline failure (UNS-CANONICAL-DESIGN §5 / D-U23): the framework-owned
 * timer fired before a reply arrived — the ephemeral reply subscription has been cleaned up
 * and the {@link ReplyFuture} rejects with this error. Java signals
 * `java.util.concurrent.TimeoutException`; Python raises `RequestTimeoutError`.
 */
export class RequestTimeoutError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "RequestTimeoutError";
  }
}

/**
 * Thrown by the reserved-class publish guard (UNS-CANONICAL-DESIGN §4.1, D-U4/D-U8/D-U24)
 * when a client-chosen topic targets a library-owned UNS class (`state | metric | cfg | log`).
 * Components must not publish to reserved classes directly — the library publishers
 * (heartbeat/state keepalive, the metric subsystem via `gg.metrics()`, the effective-config
 * publisher) own those topics and reach them through the privileged internal seam.
 *
 * The guard is misuse prevention, not a security boundary — per-device broker ACLs are the
 * durable enforcement (DESIGN-uns §7.5).
 */
export class ReservedTopicError extends Error {
  /** The rejected topic. */
  readonly topic: string;
  /** The reserved UNS class token (`state | metric | cfg | log`) that triggered the rejection. */
  readonly classToken: string;

  constructor(topic: string, classToken: string) {
    super(
      `topic '${topic}' targets the reserved UNS class '${classToken}'` +
        " (state|metric|cfg|log are library-owned): use the library publishers instead" +
        " (heartbeat/state keepalive, the metric subsystem via gg.metrics(), the" +
        " effective-config publisher)",
    );
    this.name = "ReservedTopicError";
    this.topic = topic;
    this.classToken = classToken;
  }
}

/**
 * Transport-agnostic messaging operations over {@link Message}s, with explicit
 * local / northbound method pairs (mirrors the Java/Python `IMessagingService` and
 * Rust `MessagingService`).
 */
export interface IMessagingService {
  publish(topic: string, msg: Message): Promise<void>;
  publishNorthbound(topic: string, msg: Message, qos?: Qos): Promise<void>;
  /** Strict local publish of a Message or exact encoded envelope bytes. */
  publishConfirmed?(
    topic: string,
    msgOrEncoded: Message | Buffer,
    qos: Qos,
    timeoutMs: number,
  ): Promise<void>;
  /** Strict northbound publish of a Message or exact encoded envelope bytes. */
  publishNorthboundConfirmed?(
    topic: string,
    msgOrEncoded: Message | Buffer,
    qos: Qos,
    timeoutMs: number,
  ): Promise<void>;
  publishRaw(topic: string, payload: unknown): Promise<void>;
  publishNorthboundRaw(topic: string, payload: unknown, qos?: Qos): Promise<void>;

  /**
   * Register a callback for `filter` on the local broker. `maxMessages` bounds the
   * client-side queue; `maxConcurrency` bounds simultaneous handler invocations
   * (`1` = serial, ordered).
   */
  subscribe(filter: string, handler: MessageHandler, maxMessages?: number, maxConcurrency?: number): Promise<void>;
  subscribeNorthbound(
    filter: string,
    handler: MessageHandler,
    qos?: Qos,
    maxMessages?: number,
    maxConcurrency?: number,
  ): Promise<void>;

  unsubscribe(filter: string): Promise<void>;
  unsubscribeNorthbound(filter: string): Promise<void>;

  /**
   * Send a request on the local broker; await/timeout the returned {@link ReplyFuture}.
   * `timeoutMs` semantics (§5 / D-U5): `undefined` uses the framework-owned default deadline
   * (`messaging.requestTimeoutSeconds`, default 30 s); an explicit value wins; explicit `0`
   * disables the deadline for this call. On expiry the reply subscription is cleaned up and
   * the future rejects with {@link RequestTimeoutError} — even if the caller never awaits it.
   */
  request(topic: string, msg: Message, timeoutMs?: number): ReplyFuture;
  requestNorthbound(topic: string, msg: Message, timeoutMs?: number): ReplyFuture;

  reply(request: Message, reply: Message): Promise<void>;
  replyNorthbound(request: Message, reply: Message): Promise<void>;
  /** Strict guarded reply used by the deferred registry's bounded retry loop. */
  replyConfirmed?(request: Message, reply: Message, timeoutMs: number): Promise<void>;
  /** Strict guarded northbound reply counterpart. */
  replyNorthboundConfirmed?(request: Message, reply: Message, timeoutMs: number): Promise<void>;
  /** Validate and guard a received request's reply target without publishing. */
  validateReplyTarget?(request: Message): void;

  cancelRequest(reply: ReplyFuture): void;
  cancelRequestNorthbound(reply: ReplyFuture): void;

  /**
   * Whether the underlying messaging transport is currently connected. The readiness signal behind the
   * `/readyz` health probe (FR-HB-1): the runtime is "ready" only when this is `true` (and the app's
   * `setReady` flag is set and the process is not shutting down). Cheap, non-blocking, and never
   * consulted by `/livez` (a broker outage must not fail liveness).
   */
  connected(): boolean;
}

/** Prefix for generated reply topics. Matches the other libraries exactly. */
export const REPLY_TOPIC_PREFIX = "edgecommons/reply-";
