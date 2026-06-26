/**
 * Shared test fakes/helpers reused across the new coverage suites.
 *
 * - {@link RecordingMessagingService}: an `IMessagingService` that records every
 *   publish / publishToIotCore / publishRaw call and supports a scripted request
 *   reply, for asserting routing + envelope shape without a transport.
 * - {@link FakeMessagingProvider}: an in-memory `MessagingProvider` (topic->subscribers
 *   with MQTT wildcard matching) that loops published messages back to matching
 *   subscribers, so request/reply works through the real `DefaultMessagingService`.
 * - {@link FakeIpcClient}: a fake `greengrasscoreipc.Client` subset for the IPC
 *   provider and the GG_CONFIG / SHADOW config sources.
 */
import type { Message } from "../src/message";
import { Message as Msg } from "../src/message";
import {
  Destination,
  IMessagingService,
  MessageHandler,
  MessagingProvider,
  Qos,
  RawSubscription,
  ReplyFuture,
} from "../src/messaging/types";
import { topicMatches } from "../src/messaging/standalone-provider";

/** A recorded publish call. */
export interface PublishRecord {
  kind: "publish" | "publishToIotCore" | "publishRaw" | "publishToIotCoreRaw";
  topic: string;
  message?: Message;
  payload?: unknown;
  qos?: Qos;
}

/**
 * An `IMessagingService` that records publishes and serves a scripted reply for
 * `request()` (resolving on the next microtask with `replyBody` wrapped in a
 * Message). Subscriptions are recorded and can be driven via {@link emit}.
 */
export class RecordingMessagingService implements IMessagingService {
  readonly published: PublishRecord[] = [];
  readonly subscriptions = new Map<string, MessageHandler>();
  readonly unsubscribed: string[] = [];
  /** When set, `request()` resolves with this body. When undefined it never resolves. */
  replyBody: unknown = undefined;

  async publish(topic: string, msg: Message): Promise<void> {
    this.published.push({ kind: "publish", topic, message: msg });
  }
  async publishToIotCore(topic: string, msg: Message, qos: Qos = Qos.AtLeastOnce): Promise<void> {
    this.published.push({ kind: "publishToIotCore", topic, message: msg, qos });
  }
  async publishRaw(topic: string, payload: unknown): Promise<void> {
    this.published.push({ kind: "publishRaw", topic, payload });
  }
  async publishToIotCoreRaw(topic: string, payload: unknown, qos: Qos = Qos.AtLeastOnce): Promise<void> {
    this.published.push({ kind: "publishToIotCoreRaw", topic, payload, qos });
  }

  async subscribe(filter: string, handler: MessageHandler): Promise<void> {
    this.subscriptions.set(filter, handler);
  }
  async subscribeToIotCore(filter: string, handler: MessageHandler): Promise<void> {
    this.subscriptions.set(filter, handler);
  }
  async unsubscribe(filter: string): Promise<void> {
    this.unsubscribed.push(filter);
    this.subscriptions.delete(filter);
  }
  async unsubscribeFromIotCore(filter: string): Promise<void> {
    this.unsubscribed.push(filter);
    this.subscriptions.delete(filter);
  }

  /** Deliver a message to a recorded subscription handler. */
  emit(filter: string, body: unknown): void {
    const h = this.subscriptions.get(filter);
    if (h) void h(filter, Msg.envelope({ name: "", version: "", timestamp: "", correlation_id: "", uuid: "" }, {}, body));
  }

  request(_topic: string, _msg: Message, timeoutMs = 0): ReplyFuture {
    const promise = new Promise<Message>((resolve, reject) => {
      if (this.replyBody !== undefined) {
        queueMicrotask(() =>
          resolve(Msg.envelope({ name: "Config", version: "1.0", timestamp: "", correlation_id: "", uuid: "" }, {}, this.replyBody)),
        );
      } else if (timeoutMs > 0) {
        setTimeout(() => reject(new Error("request timed out")), timeoutMs);
      }
      // else: never settles (used to exercise retries).
    });
    return new ReplyFuture(promise, () => undefined);
  }
  requestFromIotCore(topic: string, msg: Message, timeoutMs = 0): ReplyFuture {
    return this.request(topic, msg, timeoutMs);
  }
  async reply(): Promise<void> {}
  async replyToIotCore(): Promise<void> {}
  cancelRequest(reply: ReplyFuture): void {
    reply.cancel();
  }
  cancelRequestFromIotCore(reply: ReplyFuture): void {
    reply.cancel();
  }
  /** Toggle to drive the /readyz readiness signal in tests. */
  connectedState = true;
  connected(): boolean {
    return this.connectedState;
  }
}

/** A subscription registered with the {@link FakeMessagingProvider}. */
interface FakeSub {
  filter: string;
  dest: Destination;
  onMessage: (topic: string, payload: Buffer) => void;
}

/**
 * In-memory `MessagingProvider` with MQTT wildcard routing. Published bytes are
 * delivered synchronously to every matching subscriber on the same destination, so
 * a reply published to a reply-topic loops back to the requester through the real
 * `DefaultMessagingService`.
 */
export class FakeMessagingProvider implements MessagingProvider {
  readonly subs: FakeSub[] = [];
  readonly published: Array<{ topic: string; dest: Destination; qos: Qos; payload: Buffer }> = [];
  disconnected = false;

  async publishBytes(topic: string, payload: Buffer, dest: Destination, qos: Qos): Promise<void> {
    this.published.push({ topic, dest, qos, payload });
    for (const s of [...this.subs]) {
      if (s.dest === dest && topicMatches(s.filter, topic)) {
        s.onMessage(topic, payload);
      }
    }
  }

  async subscribeRaw(
    filter: string,
    dest: Destination,
    _qos: Qos,
    onMessage: (topic: string, payload: Buffer) => void,
  ): Promise<RawSubscription> {
    const sub: FakeSub = { filter, dest, onMessage };
    this.subs.push(sub);
    return {
      unsubscribe: async () => {
        const i = this.subs.indexOf(sub);
        if (i >= 0) this.subs.splice(i, 1);
      },
    };
  }

  /** Reports connected until {@link disconnect}; tests may flip {@link connectedState} directly. */
  connectedState = true;
  connected(): boolean {
    return this.connectedState && !this.disconnected;
  }

  async disconnect(): Promise<void> {
    this.disconnected = true;
    this.subs.length = 0;
  }
}

// --------------------------------------------------------------------------
// Fake Greengrass IPC client
// --------------------------------------------------------------------------

type Listener = (event: unknown) => void;

/** A fake `StreamingOperation` with `.on()` / `.activate()` / `.close()`. */
export class FakeStream {
  private readonly listeners = new Map<string, Listener[]>();
  activated = false;
  closed = false;

  on(event: string, fn: Listener): this {
    const arr = this.listeners.get(event) ?? [];
    arr.push(fn);
    this.listeners.set(event, arr);
    return this;
  }
  /** Drive an event to all registered listeners (test helper). */
  fire(event: string, payload?: unknown): void {
    for (const fn of this.listeners.get(event) ?? []) fn(payload);
  }
  async activate(): Promise<void> {
    this.activated = true;
  }
  async close(): Promise<void> {
    this.closed = true;
  }
}

/**
 * A fake `greengrasscoreipc.Client` covering exactly the subset used by
 * {@link IpcMessagingProvider} and the GG_CONFIG / SHADOW config sources. Records
 * calls and exposes the streams so tests can drive `message` events.
 */
export class FakeIpcClient {
  readonly publishedTopic: Array<{ topic: string; message: Buffer }> = [];
  readonly publishedIot: Array<{ topicName: string; qos: unknown; payload: Buffer }> = [];
  readonly topicStreams: FakeStream[] = [];
  readonly iotStreams: FakeStream[] = [];
  readonly configStreams: FakeStream[] = [];
  readonly shadowUpdates: Array<{ thingName: string; shadowName: string; payload: Buffer }> = [];
  readonly shadowDeletes: Array<{ thingName: string; shadowName: string }> = [];
  /** Bytes returned by getThingShadow; when undefined, getThingShadow rejects. */
  shadowBytes?: Buffer;
  /** Value returned by getConfiguration. */
  configValue: unknown = { a: 1 };
  closed = false;

  async publishToTopic(req: { topic: string; publishMessage: { binaryMessage?: { message?: Buffer } } }): Promise<unknown> {
    this.publishedTopic.push({ topic: req.topic, message: req.publishMessage.binaryMessage?.message as Buffer });
    return {};
  }
  async publishToIoTCore(req: { topicName: string; qos: unknown; payload: Buffer }): Promise<unknown> {
    this.publishedIot.push(req);
    return {};
  }
  subscribeToTopic(_req: unknown): FakeStream {
    const s = new FakeStream();
    this.topicStreams.push(s);
    return s;
  }
  subscribeToIoTCore(_req: unknown): FakeStream {
    const s = new FakeStream();
    this.iotStreams.push(s);
    return s;
  }
  async getConfiguration(_req: { keyPath: string[]; componentName?: string }): Promise<{ value: unknown }> {
    return { value: this.configValue };
  }
  subscribeToConfigurationUpdate(_req: unknown): FakeStream {
    const s = new FakeStream();
    this.configStreams.push(s);
    return s;
  }
  async getThingShadow(_req: { thingName: string; shadowName: string }): Promise<{ payload: Buffer }> {
    if (this.shadowBytes === undefined) throw new Error("shadow not found");
    return { payload: this.shadowBytes };
  }
  async updateThingShadow(req: { thingName: string; shadowName: string; payload: Buffer }): Promise<unknown> {
    this.shadowUpdates.push(req);
    return {};
  }
  async deleteThingShadow(req: { thingName: string; shadowName: string }): Promise<unknown> {
    this.shadowDeletes.push(req);
    return {};
  }
  async close(): Promise<void> {
    this.closed = true;
  }
}

/** A small awaiter for async settling. */
export function tick(ms = 0): Promise<void> {
  return new Promise((r) => setTimeout(r, ms));
}

/** Check whether the local MQTT broker is reachable (for self-skipping). */
export async function brokerReachable(host = "127.0.0.1", port = 1883, timeoutMs = 1000): Promise<boolean> {
  const net = await import("net");
  return new Promise<boolean>((resolve) => {
    const socket = new net.Socket();
    const done = (ok: boolean): void => {
      socket.destroy();
      resolve(ok);
    };
    socket.setTimeout(timeoutMs);
    socket.once("connect", () => done(true));
    socket.once("timeout", () => done(false));
    socket.once("error", () => done(false));
    socket.connect(port, host);
  });
}
