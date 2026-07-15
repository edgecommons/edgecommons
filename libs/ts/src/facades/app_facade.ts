/**
 * The `app()` publish facade — free-form inter-component pub/sub on the `app` class
 * (DESIGN-class-facades §2.3, D3). `app` is the intentionally-open class, so the facade's value
 * is **not** body enforcement (there is no contract to enforce) — it is removing the raw
 * three-line ritual and guaranteeing topic + identity correctness: a **named** header, the
 * developer body **verbatim**, minted onto `app/{channel}` with the envelope identity stamped.
 * `app` is non-reserved — this publishes through the ordinary guarded
 * `messaging().publish(...)`. Mirrors the Java `com.mbreissi.edgecommons.facades.AppFacade`.
 *
 * Routing: {@link Channel.LOCAL} (default) or {@link Channel.NORTHBOUND}; a `stream` route is
 * **rejected** (same reasoning as `events()`).
 *
 * **Library-internal:** obtain via `gg.instance(id).app()` or the `main` convenience `gg.app()`.
 */
import type { Config } from "../config/model";
import { sanitize } from "../config/template";
import { EdgeCommonsError } from "../errors";
import { logger } from "../logging";
import { Message, MessageBuilder } from "../message";
import type { IMessagingService } from "../messaging/types";
import { PublishConfirmationError, Qos } from "../messaging/types";
import { Uns, UnsClass } from "../uns";
import type { Channel } from "./channel";

/** The app envelope header version (the header `name` is the caller's chosen name). */
export const APP_MESSAGE_VERSION = "1.0";

/**
 * A prepared application publication with stable UUID/timestamp and defensively retained exact
 * envelope bytes for durable outbox retry.
 */
export class PreparedAppMessage {
  private constructor(
    readonly topic: string,
    private readonly preparedMessage: Message,
    private readonly bytes: Buffer,
  ) {}

  /** @internal Constructed only by {@link AppFacade}. */
  static _create(topic: string, message: Message): PreparedAppMessage {
    return new PreparedAppMessage(topic, message, Buffer.from(message.toBytes()));
  }

  /** Defensive decoded view of the prepared envelope. */
  get message(): Message {
    return Message.fromBytes(this.bytes);
  }

  /** Defensive copy of the exact serialized envelope. */
  get encodedBytes(): Buffer {
    return Buffer.from(this.bytes);
  }
}

export class AppFacade {
  /**
   * @param configProvider a snapshot accessor (envelope identity)
   * @param instanceId     the instance token this facade is bound to
   * @param uns            the instance-bound UNS topic builder
   * @param messaging      the (guarded) messaging service
   */
  constructor(
    private readonly configProvider: () => Config,
    private readonly instanceId: string | undefined,
    private readonly uns: Uns,
    private readonly messaging: IMessagingService,
  ) {}

  /**
   * Publishes a free-form message on `app/{channel}`.
   *
   * @param name    the envelope header `name` (the developer's message name; REQUIRED)
   * @param channel the `app/{channel}` tail (each `/`-token is sanitized; REQUIRED)
   * @param body    the developer body, published verbatim
   * @param routing the routing channel, or `undefined` for LOCAL
   * @throws EdgeCommonsError (kind `Validation`) when `name`/`channel` is empty or `routing` is a `stream` channel
   */
  async publish(name: string, channel: string, body: Record<string, unknown>, routing?: Channel): Promise<void> {
    const prepared = this.prepare(name, channel, body);
    await this.publishPrepared(prepared, routing);
  }

  /** Construct an application envelope without publishing it. */
  prepare(name: string, channel: string, body: Record<string, unknown>): PreparedAppMessage {
    return this.prepareInternal(name, channel, body);
  }

  /** Construct an application envelope carrying a received request or explicit correlation id. */
  prepareCorrelated(
    name: string,
    channel: string,
    body: Record<string, unknown>,
    requestOrCorrelationId: Message | string,
  ): PreparedAppMessage {
    const correlationId = typeof requestOrCorrelationId === "string"
      ? requestOrCorrelationId
      : requestOrCorrelationId?.header?.correlation_id;
    if (!correlationId) {
      throw EdgeCommonsError.validation("correlated app message requires a non-empty correlation id");
    }
    return this.prepareInternal(name, channel, body, correlationId);
  }

  private prepareInternal(
    name: string,
    channel: string,
    body: Record<string, unknown>,
    correlationId?: string,
  ): PreparedAppMessage {
    if (!name) {
      throw EdgeCommonsError.validation("app publish requires a non-empty header name");
    }
    if (!channel) {
      throw EdgeCommonsError.validation("app publish requires a non-empty channel");
    }
    const topic = this.uns.topic(UnsClass.App, channelToken(channel));
    const builder = MessageBuilder.create(name, APP_MESSAGE_VERSION)
      .withConfig(this.configProvider())
      .withInstance(this.instanceId)
      .withPayload(body);
    if (correlationId !== undefined) builder.withCorrelationId(correlationId);
    return PreparedAppMessage._create(topic, builder.build());
  }

  /** Publish a previously prepared envelope through the behavior-compatible immediate path. */
  async publishPrepared(prepared: PreparedAppMessage, routing?: Channel): Promise<void> {
    rejectStream(routing);
    if (routing !== undefined && routing.kind === "northbound") {
      try {
        await this.messaging.publishNorthbound(prepared.topic, prepared.message, Qos.AtLeastOnce);
      } catch (e) {
        logger.warn(`Northbound app publish on '${prepared.topic}' failed (local readiness unaffected): ${errMsg(e)}`);
      }
    } else {
      await this.messaging.publish(prepared.topic, prepared.message);
    }
  }

  /** Publish the exact prepared bytes and wait for positive QoS-1 acknowledgement. */
  async publishConfirmed(
    prepared: PreparedAppMessage,
    timeoutMs: number,
    routing?: Channel,
  ): Promise<void> {
    rejectStream(routing);
    if (routing !== undefined && routing.kind === "northbound") {
      const confirmed = this.messaging.publishNorthboundConfirmed;
      if (typeof confirmed !== "function") {
        throw new PublishConfirmationError("unsupported", "messaging service does not support confirmed northbound publish");
      }
      await confirmed.call(this.messaging, prepared.topic, prepared.encodedBytes, Qos.AtLeastOnce, timeoutMs);
      return;
    }
    const confirmed = this.messaging.publishConfirmed;
    if (typeof confirmed !== "function") {
      throw new PublishConfirmationError("unsupported", "messaging service does not support confirmed local publish");
    }
    await confirmed.call(this.messaging, prepared.topic, prepared.encodedBytes, Qos.AtLeastOnce, timeoutMs);
  }
}

function rejectStream(channel?: Channel): void {
  if (channel !== undefined && channel.kind === "stream") {
    throw EdgeCommonsError.validation("app() does not support the stream channel - use data() for streamed telemetry");
  }
}

/** The sanitized `app` channel token (each `/`-token → a UNS token). */
function channelToken(channel: string): string {
  return channel
    .split("/")
    .map((token) => sanitize(token))
    .join("/");
}

function errMsg(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}
