/**
 * Messaging provider — Greengrass IPC (GREENGRASS mode), over
 * `aws-iot-device-sdk-v2`'s `greengrasscoreipc` client (the V1 IPC surface).
 *
 * Implements the transport {@link MessagingProvider} (local pub/sub via
 * `publishToTopic`/`subscribeToTopic`; the IoT Core bridge via
 * `publishToIoTCore`/`subscribeToIoTCore`) and additionally exposes the Greengrass
 * config + device-shadow operations the `GG_CONFIG` / `SHADOW` config sources need.
 *
 * Wire parity: an envelope is published as a `binaryMessage`; a raw payload via the
 * service's `publishRaw` is also bytes (the service serializes). Inbound, both
 * `binaryMessage` and `jsonMessage` are normalized to bytes and the delivered topic
 * is the message's own `context.topic`. Validated on a live nucleus.
 */
import { greengrasscoreipc, eventstream_rpc } from "aws-iot-device-sdk-v2";

import { GgError } from "../errors";
import { Destination, MessagingProvider, Qos, RawSubscription } from "./types";

import model = greengrasscoreipc.model;

/** Connection options for {@link IpcMessagingProvider.connect}. */
export interface IpcOptions {
  /** Receive a component's own published messages too (default: false). */
  receiveOwnMessages?: boolean;
}

function ipcQos(qos: Qos): model.QOS {
  return qos === Qos.AtMostOnce ? model.QOS.AT_MOST_ONCE : model.QOS.AT_LEAST_ONCE;
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
    if (dest === Destination.IotCore) {
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
    if (dest === Destination.IotCore) {
      return this.subscribeIotCore(filter, qos, onMessage);
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
        if (payload !== undefined) onMessage(topic, toBuffer(payload));
      } else if (event.jsonMessage) {
        const topic = event.jsonMessage.context?.topic ?? filter;
        onMessage(topic, Buffer.from(JSON.stringify(event.jsonMessage.message ?? null), "utf8"));
      }
    });
    op.on("streamError", () => true);
    await op.activate();
    return this.track(op);
  }

  private async subscribeIotCore(
    filter: string,
    qos: Qos,
    onMessage: (topic: string, payload: Buffer) => void,
  ): Promise<RawSubscription> {
    const op = this.client.subscribeToIoTCore({ topicName: filter, qos: ipcQos(qos) });
    op.on("message", (event: model.IoTCoreMessage) => {
      const payload = event.message?.payload;
      const topic = event.message?.topicName ?? filter;
      if (payload !== undefined) onMessage(topic, toBuffer(payload));
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
    op.on("message", () => onChange());
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

  async disconnect(): Promise<void> {
    const ops = [...this.streams];
    this.streams.clear();
    await Promise.allSettled(ops.map((op) => op.close()));
    await this.client.close();
  }
}
