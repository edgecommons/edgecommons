/**
 * The `data()` publish facade — the telemetry / signal data plane (DESIGN-class-facades §2.1,
 * D2/D5). It **constructs and validates the `SouthboundSignalUpdate` body**
 * (`device`/`signal`/`samples`) so an adapter never hand-builds it, applies the body defaults,
 * sanitizes the signal path into the UNS `data` channel, stamps the envelope identity, and
 * routes to the resolved {@link Channel}. It publishes through the **ordinary, guarded**
 * `messaging().publish(...)` — `data` is non-reserved, so it passes the guard; the facade adds
 * body-contract enforcement + defaults, **not** privilege (there is no reserved-publish seam
 * here). Mirrors the Java `com.mbreissi.edgecommons.facades.DataFacade`.
 *
 * Body (`header.name` = {@link DATA_MESSAGE_NAME}, version {@link DATA_MESSAGE_VERSION}):
 * ```jsonc
 * { "device": {adapter, instance, endpoint}?,
 *   "signal": { "id": "<REQUIRED>", "name"?, "address"? },
 *   "samples": [ { "value": "<REQUIRED>", "quality", "qualityRaw"?, "sourceTs"?, "serverTs" } ] }
 * ```
 *
 * Defaulting (DESIGN-class-facades §2.1, pinned by `uns-test-vectors/data.json`):
 * 1. `quality` → {@link Quality.Good} when omitted on a sample that carries a value.
 * 2. `qualityRaw` → the synthetic marker {@link QUALITY_UNSPECIFIED} when (and only when) the
 *    quality was defaulted; else the caller's value verbatim, else absent.
 * 3. `serverTs` → now (ISO-8601 UTC `…Z`, from the injected clock) when omitted; `sourceTs` is
 *    **never** synthesized (absent when the source has none).
 * 4. The `samples` wrapper is enforced for the value-shorthand (a caller never emits a bare value).
 * 5. `signal.id` is the **only** hard reject — a publish with no stable id throws
 *    `EdgeCommonsError` (kind `Validation`) at the call site.
 *
 * Channel routing (DESIGN-class-facades §4, D1): per-call {@link SignalUpdate.via} override ▸
 * config `publish.channel` (instance ▸ global) ▸ {@link Channel.LOCAL}. A `stream:<name>` route
 * serializes the same envelope and appends it via the bound {@link StreamSink} (partition key =
 * `signal.id`, ts = `serverTs`); when streaming is not configured it falls back to a LOCAL
 * publish (readiness / no-streaming → local). Northbound / stream transport failures are caught
 * and logged — they must never flip local readiness.
 *
 * **Library-internal:** obtain the bound instance via `gg.instance(id).data()` (or the
 * `main`-instance convenience `gg.data()`); the exported constructor exists only so
 * `EdgeCommonsInstance`/`EdgeCommons` (`edgecommons.ts`) can wire it.
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
import { Quality } from "./quality";
import type { Sample, SignalUpdate } from "./signal_update";
import { SignalUpdateBuilder } from "./signal_update";
import type { StreamSink } from "./stream_sink";

/** The signal-update envelope header name (`docs/SOUTHBOUND.md` §2). */
export const DATA_MESSAGE_NAME = "SouthboundSignalUpdate";
/** The signal-update envelope header version. */
export const DATA_MESSAGE_VERSION = "1.0";
/** The `qualityRaw` marker written when `quality` was defaulted to {@link Quality.Good}. */
export const QUALITY_UNSPECIFIED = "unspecified";

export class DataFacade {
  private warnedNoStream = false;

  /**
   * @param configProvider a snapshot accessor (envelope identity + `publish.channel` lookup)
   * @param instanceId     the instance token this facade is bound to
   * @param uns            the instance-bound UNS topic builder
   * @param messaging      the (guarded) messaging service
   * @param streamSink     the stream seam, or `undefined` when streaming is not configured
   * @param clockMillis    the clock for `serverTs` defaults (injected for deterministic tests)
   */
  constructor(
    private readonly configProvider: () => Config,
    private readonly instanceId: string,
    private readonly uns: Uns,
    private readonly messaging: IMessagingService,
    private readonly streamSink: StreamSink | undefined,
    private readonly clockMillis: ClockMillis = () => Date.now(),
  ) {}

  /** The instance token this facade is bound to. */
  instanceIdValue(): string {
    return this.instanceId;
  }

  // ===================== fluent builder entry point =====================

  /**
   * Starts building a `SouthboundSignalUpdate` for a stable `signal.id` — the fluent body
   * builder that subsumes the hand-assembled JSON object. Terminate with
   * `SignalUpdateBuilder.publish()`.
   *
   * @param id the stable `signal.id` (REQUIRED at publish — the consumer key)
   */
  signal(id: string | undefined): SignalUpdateBuilder {
    return new SignalUpdateBuilder(id, (update) => this.publishUpdate(update));
  }

  // ===================== publish (shorthand + full form) =====================

  /**
   * The value-shorthand: publish one value for a signal path (the path doubles as the stable
   * `signal.id`). The single value is wrapped into a one-element `samples` array with
   * `quality=GOOD`, `qualityRaw="unspecified"`, `serverTs=now` — a caller never emits a bare value.
   */
  async publish(signalPath: string, value: unknown, quality?: Quality): Promise<void>;
  /** Publishes a built {@link SignalUpdate} (the full form; see {@link signal}/{@link SignalUpdateBuilder.build}). */
  async publish(update: SignalUpdate): Promise<void>;
  async publish(a: string | SignalUpdate, value?: unknown, quality?: Quality): Promise<void> {
    if (typeof a === "string") {
      const update = this.signal(a).addSample(value, { quality }).signalPath(a).build();
      await this.publishUpdate(update);
    } else {
      await this.publishUpdate(a);
    }
  }

  // ===================== the raw escape hatch =====================

  /**
   * The raw escape hatch (D5): publishes a caller-owned pre-built body verbatim to
   * `data/{signalPath}`, applying **no** body defaulting — only the topic + identity guarantees.
   * For a component with an exotic body the facade should not shape.
   */
  async publishBody(signalPath: string, body: Record<string, unknown>, via?: Channel): Promise<void> {
    const channel = this.channelToken(signalPath);
    const topic = this.uns.topic(UnsClass.Data, channel);
    const msg = this.message(body);
    await this.route(via, topic, msg, signalPath, firstServerTsMillis(body));
  }

  // ===================== the SignalUpdate publish path =====================

  /**
   * Validates `signal.id`, constructs the body with the defaulting rules, sanitizes the path
   * into the `data` channel, stamps the envelope, and routes to the resolved channel.
   *
   * @throws EdgeCommonsError (kind `Validation`) when `signal.id`/the path is missing/empty, there are no
   *                 samples, or a sample carries no value
   */
  private async publishUpdate(update: SignalUpdate): Promise<void> {
    if (!update.signalId) {
      throw EdgeCommonsError.validation(
        "data publish requires a stable signal.id (the consumer key) - it is the only" +
          " non-defaultable field",
      );
    }
    if (update.samples.length === 0) {
      throw EdgeCommonsError.validation("data publish requires at least one sample");
    }
    const body = this.buildBody(update);
    const channel = this.channelToken(update.signalPath ?? update.signalId);
    const topic = this.uns.topic(UnsClass.Data, channel);
    const msg = this.message(body);
    await this.route(update.via, topic, msg, update.signalId, firstServerTsMillis(body));
  }

  // ===================== body construction (THE contract) =====================

  /**
   * Constructs the wire body from a {@link SignalUpdate}, applying the §2.1 defaulting rules
   * (quality → GOOD + `qualityRaw` marker, `serverTs` → now, samples wrapper). Deterministic
   * given the injected clock — this is the exact body the vectors pin.
   *
   * @throws EdgeCommonsError (kind `Validation`) when a sample carries no value
   */
  buildBody(update: SignalUpdate): Record<string, unknown> {
    const signal: Record<string, unknown> = { id: update.signalId };
    if (update.signalName !== undefined) signal.name = update.signalName;
    if (update.signalAddress !== undefined) signal.address = update.signalAddress;

    const samples = update.samples.map((sample) => this.buildSample(sample));

    const body: Record<string, unknown> = {};
    if (update.device !== undefined) body.device = update.device;
    body.signal = signal;
    body.samples = samples;
    return body;
  }

  /** Builds one sample with the quality/qualityRaw/serverTs defaulting rules. */
  private buildSample(sample: Sample): Record<string, unknown> {
    if (sample.value === undefined) {
      throw EdgeCommonsError.validation(
        "data sample value is required (a quality-only sample is not a sample) - pass" +
          " BAD/UNCERTAIN for a failed read",
      );
    }
    const out: Record<string, unknown> = { value: sample.value };

    const qualityDefaulted = sample.quality === undefined;
    const quality = qualityDefaulted ? Quality.Good : sample.quality;
    out.quality = quality;

    let qualityRaw = sample.qualityRaw;
    if (qualityRaw === undefined && qualityDefaulted) {
      qualityRaw = QUALITY_UNSPECIFIED;
    }
    if (qualityRaw !== undefined) out.qualityRaw = qualityRaw;

    if (sample.sourceTs !== undefined) out.sourceTs = sample.sourceTs;
    out.serverTs = sample.serverTs ?? toIso(this.clockMillis());
    return out;
  }

  // ===================== channel routing =====================

  /**
   * Resolves the effective channel: per-call `via` override ▸ config `publish.channel`
   * (instance ▸ global) ▸ {@link Channel.LOCAL} (DESIGN-class-facades §4, D1).
   */
  resolveChannel(via: Channel | undefined): Channel {
    if (via !== undefined) return via;
    return this.configuredChannel() ?? Channel.LOCAL;
  }

  /**
   * Reads the config `publish.channel` default (Option C): the bound instance's
   * `publish.channel` ▸ the global `component.global.publish.channel`. Best-effort — any
   * lookup/parse anomaly yields `undefined` (fall through to LOCAL).
   */
  private configuredChannel(): Channel | undefined {
    try {
      const config = this.configProvider();
      const fromInstance = publishChannelOf(config.instance(this.instanceId));
      if (fromInstance !== undefined) return fromInstance;
      return publishChannelOf(config.global());
    } catch (e) {
      logger.debug(`publish.channel lookup failed (defaulting to LOCAL): ${errMsg(e)}`);
      return undefined;
    }
  }

  /**
   * Routes a built envelope to the resolved channel. LOCAL publishes on the guarded bus;
   * NORTHBOUND publishes to IoT Core; a stream route appends the serialized envelope to the
   * named stream (falling back to LOCAL when no sink is wired). Northbound / stream failures are
   * caught + logged (they must never flip local readiness).
   */
  private async route(
    via: Channel | undefined,
    topic: string,
    msg: Message,
    partitionKey: string,
    tsMillis: number,
  ): Promise<void> {
    const channel = this.resolveChannel(via);
    switch (channel.kind) {
      case "local":
        await this.messaging.publish(topic, msg);
        break;
      case "northbound":
        try {
          await this.messaging.publishNorthbound(topic, msg, Qos.AtLeastOnce);
        } catch (e) {
          logger.warn(`Northbound data publish on '${topic}' failed (local readiness unaffected): ${errMsg(e)}`);
        }
        break;
      case "stream":
        await this.appendToStream(channel.streamName, topic, msg, partitionKey, tsMillis);
        break;
    }
  }

  /** The `stream:<name>` route: append the serialized envelope, or fall back to LOCAL. */
  private async appendToStream(
    streamName: string,
    topic: string,
    msg: Message,
    partitionKey: string,
    tsMillis: number,
  ): Promise<void> {
    if (!this.streamSink) {
      if (!this.warnedNoStream) {
        this.warnedNoStream = true;
        logger.warn(
          `data channel 'stream:${streamName}' requested but streaming is not configured -` +
            " routing to LOCAL instead (readiness/no-streaming -> local)",
        );
      }
      await this.messaging.publish(topic, msg);
      return;
    }
    try {
      const payload = msg.toBytes();
      this.streamSink(streamName, partitionKey, tsMillis, payload);
    } catch (e) {
      logger.warn(`Stream append to 'stream:${streamName}' failed (local readiness unaffected): ${errMsg(e)}`);
    }
  }

  // ===================== helpers =====================

  /** The sanitized channel token for a signal path (each `/`-token → a UNS token). */
  channelToken(signalPath: string | undefined): string {
    if (!signalPath) {
      throw EdgeCommonsError.validation("data signal path must be non-empty");
    }
    return signalPath
      .split("/")
      .map((token) => sanitize(token))
      .join("/");
  }

  /** Builds the identity-stamped envelope with the signal-update header. */
  private message(body: unknown): Message {
    return MessageBuilder.create(DATA_MESSAGE_NAME, DATA_MESSAGE_VERSION)
      .withConfig(this.configProvider())
      .withInstance(this.instanceId)
      .withSouthboundSignalUpdate(body)
      .build();
  }
}

/** `section.publish.channel` as a {@link Channel}, or `undefined` when absent/unparseable. */
function publishChannelOf(section: unknown): Channel | undefined {
  if (section === null || typeof section !== "object") return undefined;
  const publish = (section as Record<string, unknown>).publish;
  if (publish === null || typeof publish !== "object") return undefined;
  const channel = (publish as Record<string, unknown>).channel;
  return typeof channel === "string" ? Channel.fromConfig(channel) : undefined;
}

/** The first sample's `serverTs` as epoch millis (the stream record timestamp). */
function firstServerTsMillis(body: Record<string, unknown>): number {
  const samples = body.samples;
  if (Array.isArray(samples) && samples.length > 0) {
    const first = samples[0] as Record<string, unknown>;
    if (typeof first.serverTs === "string") {
      const parsed = Date.parse(first.serverTs);
      if (!Number.isNaN(parsed)) return parsed;
    }
  }
  return Date.now();
}

function errMsg(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}
