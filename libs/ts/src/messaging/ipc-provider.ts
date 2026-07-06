/**
 * Messaging provider — Greengrass IPC (GREENGRASS mode), over
 * `aws-iot-device-sdk-v2`'s `greengrasscoreipc` client (the V1 IPC surface).
 *
 * Implements the transport {@link MessagingProvider} (local pub/sub via
 * `publishToTopic`/`subscribeToTopic`; the northbound bridge via Greengrass
 * `publishToIoTCore`/`subscribeToIoTCore`) and additionally exposes the Greengrass
 * config + device-shadow operations the `GG_CONFIG` / `SHADOW` config sources need.
 *
 * Wire parity: an envelope is published as a `binaryMessage`; a raw payload via the
 * service's `publishRaw` is also bytes (the service serializes). Inbound, both
 * `binaryMessage` and `jsonMessage` are normalized to bytes and the delivered topic
 * is the message's own `context.topic`. Validated on a live nucleus.
 */
import { greengrasscoreipc, eventstream_rpc } from "aws-iot-device-sdk-v2";

import { EdgeCommonsError } from "../errors";
import { logger } from "../logging";
import { Destination, MessagingProvider, Qos, RawSubscription } from "./types";

import model = greengrasscoreipc.model;

/**
 * Invoke an inbound-message callback with the callback's failures contained.
 *
 * Greengrass delivers IPC stream events on a single shared event-stream RPC worker
 * thread/loop; if a user (or downstream dispatch) callback throws synchronously, or
 * returns a rejected promise, the failure escapes into that worker. Under nucleus
 * crash/restart churn that has been observed to WEDGE the eventstream loop (mirrors
 * the Java fix in 6ed774c). We therefore wrap every dispatch in try/catch, attach a
 * `.catch` to any returned promise, and log + suppress so one bad message can never
 * break the subscription or surface as an unhandledRejection.
 */
function safeDispatch(topic: string, fn: () => unknown): void {
  try {
    const result = fn();
    if (result instanceof Promise) {
      result.catch((e) => logger.warn(`edgecommons: IPC message handler rejected for ${topic}: ${String(e)}`));
    }
  } catch (e) {
    logger.warn(`edgecommons: IPC message handler threw for ${topic}: ${String(e)}`);
  }
}

/** Connection options for {@link IpcMessagingProvider.connect}. */
export interface IpcOptions {
  /** Receive a component's own published messages too (default: false). */
  receiveOwnMessages?: boolean;
}

function ipcQos(qos: Qos): model.QOS {
  switch (qos) {
    case Qos.AtMostOnce:
      return model.QOS.AT_MOST_ONCE;
    case Qos.AtLeastOnce:
      return model.QOS.AT_LEAST_ONCE;
    case Qos.ExactlyOnce:
      throw EdgeCommonsError.config("Greengrass IoT Core IPC supports only MQTT QoS 0 and 1; got ExactlyOnce");
  }
}

/** Normalize an eventstream payload (string | ArrayBuffer | view) to a Buffer. */
function toBuffer(payload: string | ArrayBuffer | ArrayBufferView): Buffer {
  if (typeof payload === "string") return Buffer.from(payload, "utf8");
  if (payload instanceof ArrayBuffer) return Buffer.from(payload);
  return Buffer.from(payload.buffer, payload.byteOffset, payload.byteLength);
}

/** Greengrass IPC transport provider plus config/shadow operations. */
export class IpcMessagingProvider implements MessagingProvider {
  private readonly streams = new Set<{ close(): Promise<void> }>();
  /** True once the IPC client is built/connected; flipped false on {@link disconnect}. */
  private open = true;

  private constructor(
    private readonly client: greengrasscoreipc.Client,
    private readonly receiveMode: model.ReceiveMode,
  ) {}

  /** Connect to the Greengrass nucleus, resolving once IPC is established. */
  static async connect(opts: IpcOptions = {}): Promise<IpcMessagingProvider> {
    const client = greengrasscoreipc.createClient();
    await client.connect();
    const receiveMode = opts.receiveOwnMessages
      ? model.ReceiveMode.RECEIVE_ALL_MESSAGES
      : model.ReceiveMode.RECEIVE_MESSAGES_FROM_OTHERS;
    return new IpcMessagingProvider(client, receiveMode);
  }

  /**
   * TESTABILITY SEAM: construct a provider around an already-built IPC client,
   * bypassing the live `createClient()`/`connect()` handshake. Lets tests inject a
   * fake client to cover the transport + config/shadow paths without a nucleus. The
   * real {@link connect} is unchanged; this adds no new runtime behavior.
   */
  static _withClient(
    client: greengrasscoreipc.Client,
    receiveMode: model.ReceiveMode,
  ): IpcMessagingProvider {
    return new IpcMessagingProvider(client, receiveMode);
  }

  /** The raw IPC client (escape hatch for advanced use). */
  nativeClient(): greengrasscoreipc.Client {
    return this.client;
  }

  async publishBytes(topic: string, payload: Buffer, dest: Destination, qos: Qos): Promise<void> {
    if (dest === Destination.Northbound) {
      await this.client.publishToIoTCore({ topicName: topic, qos: ipcQos(qos), payload });
    } else {
      await this.client.publishToTopic({ topic, publishMessage: { binaryMessage: { message: payload } } });
    }
  }

  async subscribeRaw(
    filter: string,
    dest: Destination,
    qos: Qos,
    onMessage: (topic: string, payload: Buffer) => void,
  ): Promise<RawSubscription> {
    if (dest === Destination.Northbound) {
      return this.subscribeNorthboundRaw(filter, qos, onMessage);
    }
    return this.subscribeLocal(filter, onMessage);
  }

  private async subscribeLocal(
    filter: string,
    onMessage: (topic: string, payload: Buffer) => void,
  ): Promise<RawSubscription> {
    const op = this.client.subscribeToTopic({ topic: filter, receiveMode: this.receiveMode });
    op.on("message", (event: model.SubscriptionResponseMessage) => {
      if (event.binaryMessage) {
        const topic = event.binaryMessage.context?.topic ?? filter;
        const payload = event.binaryMessage.message;
        if (payload !== undefined) safeDispatch(topic, () => onMessage(topic, toBuffer(payload)));
      } else if (event.jsonMessage) {
        const topic = event.jsonMessage.context?.topic ?? filter;
        safeDispatch(topic, () => onMessage(topic, Buffer.from(JSON.stringify(event.jsonMessage!.message ?? null), "utf8")));
      }
    });
    op.on("streamError", () => true);
    await op.activate();
    return this.track(op);
  }

  private async subscribeNorthboundRaw(
    filter: string,
    qos: Qos,
    onMessage: (topic: string, payload: Buffer) => void,
  ): Promise<RawSubscription> {
    const op = this.client.subscribeToIoTCore({ topicName: filter, qos: ipcQos(qos) });
    op.on("message", (event: model.IoTCoreMessage) => {
      const payload = event.message?.payload;
      const topic = event.message?.topicName ?? filter;
      if (payload !== undefined) safeDispatch(topic, () => onMessage(topic, toBuffer(payload)));
    });
    op.on("streamError", () => true);
    await op.activate();
    return this.track(op);
  }

  private track(op: { close(): Promise<void> }): RawSubscription {
    this.streams.add(op);
    return {
      unsubscribe: async () => {
        this.streams.delete(op);
        await op.close().catch(() => undefined);
      },
    };
  }

  // ----- Greengrass config + device-shadow operations (for config sources) -----

  /** Fetch a configuration value at `keyPath` (whole component config when empty). */
  async getConfiguration(keyPath: string[], componentName?: string): Promise<unknown> {
    const resp = await this.client.getConfiguration({ keyPath, componentName });
    return resp.value;
  }

  /**
   * Watch a configuration key path; invoke `onChange` after each update (the caller
   * re-fetches via {@link getConfiguration}). Returns a closeable subscription.
   */
  async watchConfiguration(
    keyPath: string[],
    componentName: string | undefined,
    onChange: () => void,
  ): Promise<RawSubscription> {
    const op = this.client.subscribeToConfigurationUpdate({ keyPath, componentName });
    op.on("message", () => safeDispatch(keyPath.join("/"), () => onChange()));
    op.on("streamError", () => true);
    await op.activate();
    return this.track(op);
  }

  /** Get a thing shadow document as bytes. */
  async getThingShadow(thingName: string, shadowName?: string): Promise<Buffer> {
    const resp = await this.client.getThingShadow({ thingName, shadowName: shadowName ?? "" });
    return toBuffer(resp.payload);
  }

  /** Update a thing shadow with a JSON document (bytes). */
  async updateThingShadow(thingName: string, shadowName: string | undefined, payload: Buffer): Promise<void> {
    await this.client.updateThingShadow({ thingName, shadowName: shadowName ?? "", payload });
  }

  /** Delete a thing shadow. */
  async deleteThingShadow(thingName: string, shadowName?: string): Promise<void> {
    await this.client.deleteThingShadow({ thingName, shadowName: shadowName ?? "" });
  }

  /**
   * Whether the IPC transport is connected (FR-HB-1). The Nucleus IPC client is a connected domain
   * socket from the moment {@link connect} returns it, so this is `true` until {@link disconnect}; the
   * SDK exposes no finer-grained liveness signal.
   */
  connected(): boolean {
    return this.open;
  }

  async disconnect(): Promise<void> {
    this.open = false;
    const ops = [...this.streams];
    this.streams.clear();
    await Promise.allSettled(ops.map((op) => op.close()));
    await this.client.close();
  }
}
