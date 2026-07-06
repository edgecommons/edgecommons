/**
 * Messaging — the user-facing service over any {@link MessagingProvider}.
 *
 * {@link DefaultMessagingService} adds message (de)serialization, the
 * callback-dispatch model (bounded queue + bounded concurrency), request/reply
 * correlation with a framework-owned deadline (UNS-CANONICAL-DESIGN §5), and the
 * reserved-class publish guard (§4.1) on top of the raw transport. Mirrors the
 * Rust `DefaultMessagingService` / Java `MessagingClient`.
 */
import { randomUUID } from "crypto";

import { logger } from "../logging";
import { Message } from "../message";
import { reservedClassOf } from "../uns";
import {
  Destination,
  IMessagingService,
  MessageHandler,
  MessagingProvider,
  Qos,
  RawSubscription,
  ReplyFuture,
  RequestTimeoutError,
  ReservedTopicError,
  REPLY_TOPIC_PREFIX,
} from "./types";

/** Default QoS for local operations (the Java/Python contract has no explicit QoS). */
const LOCAL_QOS = Qos.AtLeastOnce;

/**
 * The built-in default `request()` deadline (ms) that applies before the config model is
 * late-bound (§5 / D-U5) — deliberately non-zero, so the CONFIG_COMPONENT bootstrap request
 * gets a deadline instead of hanging forever.
 */
const BUILT_IN_REQUEST_TIMEOUT_MS = 30_000;

/** A bounded, concurrency-limited dispatcher draining one subscription. */
class Dispatcher {
  private readonly queue: Array<[string, Buffer]> = [];
  private active = 0;
  private closed = false;

  constructor(
    private readonly handler: MessageHandler,
    private readonly maxMessages: number,
    private readonly maxConcurrency: number,
  ) {}

  /** Enqueue a raw message, dropping (with a warning) on overflow. */
  offer(topic: string, payload: Buffer): void {
    if (this.closed) return;
    if (this.queue.length >= this.maxMessages) {
      // eslint-disable-next-line no-console
      console.warn(`edgecommons: subscription queue full (${this.maxMessages}); dropping message on ${topic}`);
      return;
    }
    this.queue.push([topic, payload]);
    this.pump();
  }

  private pump(): void {
    while (!this.closed && this.active < this.maxConcurrency && this.queue.length > 0) {
      const [topic, payload] = this.queue.shift()!;
      this.active++;
      void this.run(topic, payload);
    }
  }

  private async run(topic: string, payload: Buffer): Promise<void> {
    try {
      await this.handler(topic, Message.fromWire(payload));
    } catch (e) {
      // eslint-disable-next-line no-console
      console.warn(`edgecommons: message handler threw for ${topic}: ${String(e)}`);
    } finally {
      this.active--;
      this.pump();
    }
  }

  close(): void {
    this.closed = true;
    this.queue.length = 0;
  }
}

/** Default {@link IMessagingService} built over a {@link MessagingProvider}. */
export class DefaultMessagingService implements IMessagingService {
  /** Subscriptions keyed by `${dest} ${filter}`. */
  private readonly subscriptions = new Map<string, { sub: RawSubscription; dispatcher: Dispatcher }>();

  /**
   * Whether the reserved-class publish guard also checks the class token at topic position 5 —
   * this component's **effective** `topic.includeRoot` (§4.1, D-U24/D-U27). Late-bound by the
   * runtime right after the config loads; default `false` before that (nothing publishes
   * rooted topics pre-config).
   */
  private guardIncludeRoot = false;

  /**
   * The default `request()` deadline in ms (§5 / D-U5). Late-bound from
   * `messaging.requestTimeoutSeconds` right after the config loads; until then the built-in
   * 30 s applies — deliberately, so the CONFIG_COMPONENT bootstrap request gets a deadline
   * instead of hanging. `0` = disabled.
   */
  private defaultRequestTimeoutMs = BUILT_IN_REQUEST_TIMEOUT_MS;

  constructor(private readonly provider: MessagingProvider) {}

  private static key(dest: Destination, filter: string): string {
    return `${dest} ${filter}`;
  }

  /**
   * Late-binds the default `request()` deadline from the config model
   * (`messaging.requestTimeoutSeconds`, §5/D-U5). `0` disables the default deadline. An
   * explicit per-call timeout on `request()` always wins over this default.
   */
  setDefaultRequestTimeout(timeoutMs: number): void {
    this.defaultRequestTimeoutMs = Math.max(0, timeoutMs);
    logger.debug(`edgecommons: default request timeout bound to ${this.defaultRequestTimeoutMs} ms`);
  }

  /** The default `request()` deadline currently in effect (ms; `0` = disabled). */
  getDefaultRequestTimeout(): number {
    return this.defaultRequestTimeoutMs;
  }

  /**
   * Late-binds the reserved-class guard's `topic.includeRoot` flag from the config model
   * (§4.1, D-U24). Bind the **effective** root — `includeRoot && hier.length >= 2` (D-U27) —
   * so the guard's position-5 check agrees with topic-building, which no-ops includeRoot on a
   * single-level hierarchy (D-U25).
   */
  setGuardIncludeRoot(includeRoot: boolean): void {
    this.guardIncludeRoot = includeRoot;
    logger.debug(`edgecommons: reserved-topic guard includeRoot bound to ${includeRoot}`);
  }

  /**
   * The reserved-class publish guard (§4.1): rejects a client-chosen topic whose class
   * position holds a reserved token (`state | metric | cfg | log`). Non-`ecv1` topics pass
   * untouched; `subscribe*` is never guarded (consumers must read reserved classes).
   *
   * @throws ReservedTopicError when the topic targets a reserved UNS class
   */
  private checkReservedTopic(topic: string | undefined): void {
    const reserved = reservedClassOf(topic, this.guardIncludeRoot);
    if (reserved !== undefined) {
      throw new ReservedTopicError(topic as string, reserved);
    }
  }

  async publish(topic: string, msg: Message): Promise<void> {
    this.checkReservedTopic(topic);
    await this.provider.publishBytes(topic, Buffer.from(msg.toJSON(), "utf8"), Destination.Local, LOCAL_QOS);
  }

  async publishToIoTCore(topic: string, msg: Message, qos: Qos = Qos.AtLeastOnce): Promise<void> {
    this.checkReservedTopic(topic);
    await this.provider.publishBytes(topic, Buffer.from(msg.toJSON(), "utf8"), Destination.IoTCore, qos);
  }

  async publishRaw(topic: string, payload: unknown): Promise<void> {
    this.checkReservedTopic(topic);
    await this.provider.publishBytes(topic, Buffer.from(JSON.stringify(payload), "utf8"), Destination.Local, LOCAL_QOS);
  }

  async publishToIoTCoreRaw(topic: string, payload: unknown, qos: Qos = Qos.AtLeastOnce): Promise<void> {
    this.checkReservedTopic(topic);
    await this.provider.publishBytes(topic, Buffer.from(JSON.stringify(payload), "utf8"), Destination.IoTCore, qos);
  }

  /**
   * @internal Unguarded local publish — the privileged internal-publish seam
   * (UNS-CANONICAL-DESIGN §4.2, D-U4) for the library's own publishers (heartbeat/state
   * keepalive, the `messaging` metric target, the effective-config publisher). Component code
   * must not call this: the guard it bypasses keeps the library-owned UNS classes consistent
   * (broker ACLs are the security boundary). Stripped from the published typings
   * (`stripInternal`).
   */
  async publishReserved(topic: string, msg: Message): Promise<void> {
    await this.provider.publishBytes(topic, Buffer.from(msg.toJSON(), "utf8"), Destination.Local, LOCAL_QOS);
  }

  /** @internal Unguarded raw local publish — the privileged seam (§4.2). */
  async publishReservedRaw(topic: string, payload: unknown): Promise<void> {
    await this.provider.publishBytes(topic, Buffer.from(JSON.stringify(payload), "utf8"), Destination.Local, LOCAL_QOS);
  }

  /** @internal Unguarded IoT Core publish — the privileged seam (§4.2). */
  async publishReservedToIoTCore(topic: string, msg: Message, qos: Qos = Qos.AtLeastOnce): Promise<void> {
    await this.provider.publishBytes(topic, Buffer.from(msg.toJSON(), "utf8"), Destination.IoTCore, qos);
  }

  async subscribe(filter: string, handler: MessageHandler, maxMessages = 32, maxConcurrency = 1): Promise<void> {
    await this.startSubscription(filter, Destination.Local, LOCAL_QOS, handler, maxMessages, maxConcurrency);
  }

  async subscribeToIoTCore(
    filter: string,
    handler: MessageHandler,
    qos: Qos = Qos.AtLeastOnce,
    maxMessages = 32,
    maxConcurrency = 1,
  ): Promise<void> {
    await this.startSubscription(filter, Destination.IoTCore, qos, handler, maxMessages, maxConcurrency);
  }

  private async startSubscription(
    filter: string,
    dest: Destination,
    qos: Qos,
    handler: MessageHandler,
    maxMessages: number,
    maxConcurrency: number,
  ): Promise<void> {
    const dispatcher = new Dispatcher(handler, Math.max(maxMessages, 1), Math.max(maxConcurrency, 1));
    const sub = await this.provider.subscribeRaw(filter, dest, qos, (t, p) => dispatcher.offer(t, p));
    const key = DefaultMessagingService.key(dest, filter);
    const prev = this.subscriptions.get(key);
    if (prev) {
      prev.dispatcher.close();
      await prev.sub.unsubscribe().catch(() => undefined);
    }
    this.subscriptions.set(key, { sub, dispatcher });
  }

  async unsubscribe(filter: string): Promise<void> {
    await this.stopSubscription(filter, Destination.Local);
  }

  async unsubscribeFromIoTCore(filter: string): Promise<void> {
    await this.stopSubscription(filter, Destination.IoTCore);
  }

  private async stopSubscription(filter: string, dest: Destination): Promise<void> {
    const key = DefaultMessagingService.key(dest, filter);
    const entry = this.subscriptions.get(key);
    if (entry) {
      this.subscriptions.delete(key);
      entry.dispatcher.close();
      await entry.sub.unsubscribe().catch(() => undefined);
    }
  }

  request(topic: string, msg: Message, timeoutMs?: number): ReplyFuture {
    this.checkReservedTopic(topic);
    return this.startRequest(topic, msg, Destination.Local, LOCAL_QOS, timeoutMs ?? this.defaultRequestTimeoutMs);
  }

  requestFromIoTCore(topic: string, msg: Message, timeoutMs?: number): ReplyFuture {
    this.checkReservedTopic(topic);
    return this.startRequest(topic, msg, Destination.IoTCore, Qos.AtLeastOnce, timeoutMs ?? this.defaultRequestTimeoutMs);
  }

  private startRequest(topic: string, msg: Message, dest: Destination, qos: Qos, timeoutMs: number): ReplyFuture {
    const replyTopic = `${REPLY_TOPIC_PREFIX}${randomUUID()}`;
    msg.header.reply_to = replyTopic;

    let settled = false;
    let cleanup = (): void => undefined;
    const promise = new Promise<Message>((resolve, reject) => {
      let timer: NodeJS.Timeout | undefined;
      // The single idempotent settle path (§5): reply-arrival, deadline, and cancelRequest all
      // race through here; the loser no-ops. The reply subscription is unsubscribed from the
      // SAME destination it was subscribed on before the future settles.
      const finish = (fn: () => void): void => {
        if (settled) return;
        settled = true;
        if (timer) clearTimeout(timer);
        this.stopSubscription(replyTopic, dest).finally(fn);
      };
      cleanup = () => finish(() => reject(new Error("request canceled")));
      if (timeoutMs > 0) {
        timer = setTimeout(
          () =>
            finish(() =>
              reject(new RequestTimeoutError(`request on '${topic}' timed out after ${timeoutMs} ms`)),
            ),
          timeoutMs,
        );
        // Never keep the event loop alive solely for a pending request deadline.
        if (typeof timer.unref === "function") timer.unref();
      }
      const handler: MessageHandler = (t, reply) => {
        // Guard the late/duplicate-reply case: once the request has settled (a
        // reply arrived, or it timed out / was canceled) the reply subscription
        // has been torn down, but a straggler can still be delivered. There is no
        // pending resolver to complete, so log + drop it instead of letting it
        // fall through (mirrors the Java null-future guard in 6ed774c).
        if (settled) {
          logger.debug(`edgecommons: dropping stray reply on ${t} (request already settled)`);
          return;
        }
        finish(() => resolve(reply));
      };
      this.startSubscription(replyTopic, dest, qos, handler, 1, 1)
        .then(() =>
          this.provider.publishBytes(topic, Buffer.from(msg.toJSON(), "utf8"), dest, qos),
        )
        .catch((err) => finish(() => reject(err)));
    });

    return new ReplyFuture(promise, () => cleanup());
  }

  /**
   * Sends a reply to a received request message. The request's `reply_to` topic is guarded
   * like a client-chosen topic (§4.1, D-U8): a hostile requester could otherwise set
   * `header.reply_to` to a victim's reserved topic and turn an innocent responder into a
   * forger.
   *
   * @throws ReservedTopicError when the request's reply topic targets a reserved UNS class
   */
  async reply(request: Message, reply: Message): Promise<void> {
    this.checkReservedTopic(request.getReplyTo());
    await this.sendReply(request, reply, Destination.Local, LOCAL_QOS);
  }

  /** IoT Core variant of {@link reply} — the request's `reply_to` topic is guarded the same way. */
  async replyToIoTCore(request: Message, reply: Message): Promise<void> {
    this.checkReservedTopic(request.getReplyTo());
    await this.sendReply(request, reply, Destination.IoTCore, Qos.AtLeastOnce);
  }

  private async sendReply(request: Message, reply: Message, dest: Destination, qos: Qos): Promise<void> {
    const replyTo = request.getReplyTo();
    if (!replyTo) {
      throw new Error("cannot reply: request has no reply_to");
    }
    reply.header.correlation_id = request.getCorrelationId();
    await this.provider.publishBytes(replyTo, Buffer.from(reply.toJSON(), "utf8"), dest, qos);
  }

  cancelRequest(reply: ReplyFuture): void {
    reply.cancel();
  }

  cancelRequestFromIoTCore(reply: ReplyFuture): void {
    reply.cancel();
  }

  /** Whether the underlying transport reports a live connection (the `/readyz` signal; FR-HB-1). */
  connected(): boolean {
    return this.provider.connected();
  }

  /** Close all subscriptions and the underlying transport. */
  async disconnect(): Promise<void> {
    for (const { dispatcher } of this.subscriptions.values()) {
      dispatcher.close();
    }
    this.subscriptions.clear();
    await this.provider.disconnect();
  }
}

/**
 * @internal Publish through the privileged reserved seam when the service exposes it
 * (`DefaultMessagingService.publishReserved*`, §4.2), else fall back to the public path (test
 * fakes and custom `IMessagingService` implementations have no guard to bypass). Used by the
 * library-owned publishers (heartbeat/state, the `messaging` metric target, the
 * effective-config publisher).
 */
export async function publishReservedVia(
  svc: IMessagingService,
  topic: string,
  msg: Message,
  destination: "local" | "iotcore" = "local",
): Promise<void> {
  const seam = svc as Partial<
    Pick<DefaultMessagingService, "publishReserved" | "publishReservedToIoTCore">
  >;
  if (destination === "iotcore") {
    if (typeof seam.publishReservedToIoTCore === "function") {
      await seam.publishReservedToIoTCore(topic, msg, Qos.AtLeastOnce);
    } else {
      await svc.publishToIoTCore(topic, msg, Qos.AtLeastOnce);
    }
    return;
  }
  if (typeof seam.publishReserved === "function") {
    await seam.publishReserved(topic, msg);
  } else {
    await svc.publish(topic, msg);
  }
}
