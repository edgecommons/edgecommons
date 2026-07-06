/**
 * The `events()` publish facade — operator events & alarms on the `evt` class
 * (DESIGN-class-facades §2.2, D8). It is the facade that **stops the §1.2 `evt` drift**: it
 * makes the `evt/{severity}/{type}` channel and the body shape non-negotiable by **deriving the
 * channel from the body's own `severity` + `type`**, so the topic and body can never disagree
 * (today each adapter sets them independently). `evt` is non-reserved — this publishes through
 * the ordinary guarded `messaging().publish(...)`. Mirrors the Java
 * `com.mbreissi.edgecommons.facades.EventsFacade`.
 *
 * Body (`header.name` = {@link EVT_MESSAGE_NAME}, version {@link EVT_MESSAGE_VERSION}):
 * ```jsonc
 * { "severity": "critical|warning|info|debug",  // REQUIRED (channel token 1)
 *   "type":      "<REQUIRED>",                   // the event type (channel token 2, sanitized)
 *   "message":   "<str>?",                        // optional operator text
 *   "timestamp": "<iso>",                         // DEFAULTED to now
 *   "context":   {}?,                              // optional structured data
 *   "alarm":     "<bool>?",  "active": "<bool>?" } // present only for raiseAlarm/clearAlarm
 * ```
 *
 * Channel: `evt/{severity}/{sanitize(type)}` (2 tokens). Routing: {@link Channel.LOCAL} (default)
 * or {@link Channel.NORTHBOUND} via {@link via} — alarms often go straight to the cloud control
 * plane. A `stream` route is **rejected** (events are low-rate control-plane, not bulk telemetry).
 *
 * **TS-idiom divergence from Java:** Java overloads `emit(severity, type, message, context)` /
 * `emit(severity, type, message)` / `emit(type, message)` (severity defaults to INFO) purely by
 * arity — fragile in a structurally-typed language, so this collapses to one
 * `emit(severity, type, message?, context?)` plus a separately-named {@link emitInfo} convenience.
 * Likewise `raiseAlarm`/`clearAlarm` take `severity` as a trailing optional parameter (default
 * `CRITICAL`) instead of a leading-overload variant.
 *
 * **Library-internal:** obtain via `gg.instance(id).events()` or the `main` convenience
 * `gg.events()`.
 */
import type { Config } from "../config/model";
import { sanitize } from "../config/template";
import { EdgeCommonsError } from "../errors";
import { logger } from "../logging";
import type { Message } from "../message";
import { MessageBuilder } from "../message";
import type { IMessagingService } from "../messaging/types";
import { Qos } from "../messaging/types";
import { Uns, UnsClass } from "../uns";
import { Channel } from "./channel";
import { type ClockMillis, toIso } from "./clock";
import { Severity } from "./severity";

/** The event envelope header name. */
export const EVT_MESSAGE_NAME = "evt";
/** The event envelope header version. */
export const EVT_MESSAGE_VERSION = "1.0";

export class EventsFacade {
  /**
   * @param configProvider a snapshot accessor (envelope identity)
   * @param instanceId     the instance token this facade is bound to
   * @param uns            the instance-bound UNS topic builder
   * @param messaging      the (guarded) messaging service
   * @param clockMillis    the clock for the `timestamp` default (injected for tests)
   * @param override       the per-view routing override (set by {@link via}), or `undefined` for LOCAL
   */
  constructor(
    private readonly configProvider: () => Config,
    private readonly instanceId: string,
    private readonly uns: Uns,
    private readonly messaging: IMessagingService,
    private readonly clockMillis: ClockMillis = () => Date.now(),
    private readonly override?: Channel,
  ) {}

  /**
   * Returns a channel-bound view for a per-call routing override (LOCAL or NORTHBOUND).
   *
   * @throws EdgeCommonsError (kind `Validation`) when `channel` is a `stream` channel
   */
  via(channel: Channel): EventsFacade {
    rejectStream(channel, "events()");
    return new EventsFacade(this.configProvider, this.instanceId, this.uns, this.messaging, this.clockMillis, channel);
  }

  // ===================== emit =====================

  /**
   * Emits a one-shot event with an explicit severity, optional message, and structured context.
   *
   * @param severity the severity (channel token 1; REQUIRED)
   * @param type     the event type (channel token 2; REQUIRED)
   * @param message  optional operator text
   * @param context  optional structured data
   */
  async emit(severity: Severity, type: string, message?: string, context?: Record<string, unknown>): Promise<void> {
    await this.publish(severity, type, message, context, undefined, undefined);
  }

  /** Message-only convenience — severity defaults to {@link Severity.Info}. */
  async emitInfo(type: string, message?: string): Promise<void> {
    await this.emit(Severity.Info, type, message);
  }

  // ===================== alarms =====================

  /**
   * Raises a stateful alarm (`alarm=true, active=true`). `severity` defaults to
   * {@link Severity.Critical} so raises and clears of the same alarm ride the same
   * `evt/critical/{type}` channel (subsumes OPC UA's `connection-lost`).
   *
   * @param type     the alarm type (channel token 2)
   * @param message  optional operator text
   * @param context  optional structured data
   * @param severity the severity (defaults to {@link Severity.Critical})
   */
  async raiseAlarm(
    type: string,
    message?: string,
    context?: Record<string, unknown>,
    severity: Severity = Severity.Critical,
  ): Promise<void> {
    await this.publish(severity, type, message, context, true, true);
  }

  /**
   * Clears a stateful alarm (`alarm=true, active=false`). `severity` defaults to
   * {@link Severity.Critical} so the clear tracks on the same channel as the raise (subsumes
   * OPC UA's `connection-restored`).
   *
   * @param type     the alarm type (must match the raise's type)
   * @param context  optional structured data
   * @param severity the severity (defaults to {@link Severity.Critical})
   */
  async clearAlarm(type: string, context?: Record<string, unknown>, severity: Severity = Severity.Critical): Promise<void> {
    await this.publish(severity, type, undefined, context, true, false);
  }

  // ===================== body construction + routing =====================

  /**
   * Constructs the `evt` wire body — the exact body the vectors pin. Deterministic given the
   * injected clock. Member order: severity, type, message?, timestamp, context?, alarm?, active?.
   *
   * @throws EdgeCommonsError (kind `Validation`) when `type` is empty
   */
  buildBody(
    severity: Severity,
    type: string,
    message: string | undefined,
    context: Record<string, unknown> | undefined,
    alarm: boolean | undefined,
    active: boolean | undefined,
  ): Record<string, unknown> {
    if (!type) {
      throw EdgeCommonsError.validation("evt requires a non-empty type (it is a channel token and the event's kind)");
    }
    const body: Record<string, unknown> = { severity, type };
    if (message !== undefined) body.message = message;
    body.timestamp = toIso(this.clockMillis());
    if (context !== undefined) body.context = context;
    if (alarm !== undefined) {
      body.alarm = alarm;
      body.active = active;
    }
    return body;
  }

  /** The `evt/{severity}/{type}` channel derived from the body's own severity + type. */
  channelFor(severity: Severity, type: string): string {
    if (!type) {
      throw EdgeCommonsError.validation("evt requires a non-empty type");
    }
    return `${severity}/${sanitize(type)}`;
  }

  private async publish(
    severity: Severity,
    type: string,
    message: string | undefined,
    context: Record<string, unknown> | undefined,
    alarm: boolean | undefined,
    active: boolean | undefined,
  ): Promise<void> {
    const body = this.buildBody(severity, type, message, context, alarm, active);
    const channel = this.channelFor(severity, type);
    const topic = this.uns.topic(UnsClass.Evt, channel);
    const msg = MessageBuilder.create(EVT_MESSAGE_NAME, EVT_MESSAGE_VERSION)
      .withConfig(this.configProvider())
      .withInstance(this.instanceId)
      .withPayload(body)
      .build();
    await this.route(topic, msg);
  }

  /** LOCAL (default) or NORTHBOUND; a stream override is rejected up front by {@link via}. */
  private async route(topic: string, msg: Message): Promise<void> {
    const channel = this.override ?? Channel.LOCAL;
    if (channel.kind === "northbound") {
      try {
        await this.messaging.publishToIoTCore(topic, msg, Qos.AtLeastOnce);
      } catch (e) {
        logger.warn(`Northbound evt publish on '${topic}' failed (local readiness unaffected): ${errMsg(e)}`);
      }
    } else {
      await this.messaging.publish(topic, msg);
    }
  }
}

function rejectStream(channel: Channel | undefined, facadeName: string): void {
  if (channel !== undefined && channel.kind === "stream") {
    throw EdgeCommonsError.validation(
      `${facadeName} does not support the stream channel - events are low-rate control-plane,` +
        " not bulk telemetry (use data() for streamed telemetry)",
    );
  }
}

function errMsg(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}
