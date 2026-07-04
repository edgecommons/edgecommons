/**
 * The `app()` publish facade — free-form inter-component pub/sub on the `app` class
 * (DESIGN-class-facades §2.3, D3). `app` is the intentionally-open class, so the facade's value
 * is **not** body enforcement (there is no contract to enforce) — it is removing the raw
 * three-line ritual and guaranteeing topic + identity correctness: a **named** header, the
 * developer body **verbatim**, minted onto `app/{channel}` with the envelope identity stamped.
 * `app` is non-reserved — this publishes through the ordinary guarded
 * `messaging().publish(...)`. Mirrors the Java `com.mbreissi.ggcommons.facades.AppFacade`.
 *
 * Routing: {@link Channel.LOCAL} (default) or {@link Channel.NORTHBOUND}; a `stream` route is
 * **rejected** (same reasoning as `events()`).
 *
 * **Library-internal:** obtain via `gg.instance(id).app()` or the `main` convenience `gg.app()`.
 */
import type { Config } from "../config/model";
import { sanitize } from "../config/template";
import { GgError } from "../errors";
import { logger } from "../logging";
import { MessageBuilder } from "../message";
import type { IMessagingService } from "../messaging/types";
import { Qos } from "../messaging/types";
import { Uns, UnsClass } from "../uns";
import type { Channel } from "./channel";

/** The app envelope header version (the header `name` is the caller's chosen name). */
export const APP_MESSAGE_VERSION = "1.0";

export class AppFacade {
  /**
   * @param configProvider a snapshot accessor (envelope identity)
   * @param instanceId     the instance token this facade is bound to
   * @param uns            the instance-bound UNS topic builder
   * @param messaging      the (guarded) messaging service
   */
  constructor(
    private readonly configProvider: () => Config,
    private readonly instanceId: string,
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
   * @throws GgError (kind `Validation`) when `name`/`channel` is empty or `routing` is a `stream` channel
   */
  async publish(name: string, channel: string, body: Record<string, unknown>, routing?: Channel): Promise<void> {
    if (!name) {
      throw GgError.validation("app publish requires a non-empty header name");
    }
    if (!channel) {
      throw GgError.validation("app publish requires a non-empty channel");
    }
    if (routing !== undefined && routing.kind === "stream") {
      throw GgError.validation("app() does not support the stream channel - use data() for streamed telemetry");
    }
    const topic = this.uns.topic(UnsClass.App, channelToken(channel));
    const msg = MessageBuilder.create(name, APP_MESSAGE_VERSION)
      .withConfig(this.configProvider())
      .withInstance(this.instanceId)
      .withPayload(body)
      .build();
    if (routing !== undefined && routing.kind === "northbound") {
      try {
        await this.messaging.publishToIoTCore(topic, msg, Qos.AtLeastOnce);
      } catch (e) {
        logger.warn(`Northbound app publish on '${topic}' failed (local readiness unaffected): ${errMsg(e)}`);
      }
    } else {
      await this.messaging.publish(topic, msg);
    }
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
