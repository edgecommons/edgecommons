/**
 * Messaging — Message model (TypeScript).
 *
 * A `Message` is the unit exchanged over any transport. Its JSON shape is kept
 * byte-compatible with the Java, Python and Rust libraries so all four
 * implementations interoperate on the same topics:
 *
 * ```json
 * { "header":   { "name", "version", "timestamp", "correlation_id", "uuid", "reply_to" },
 *   "identity": { "hier": [{"level","value"}...], "path", "component", "instance" },
 *   "tags":     { "...": "..." },
 *   "body":     <any JSON> }
 * ```
 *
 * Header keys are **snake_case** (`correlation_id`, `reply_to`) to match the
 * other libraries' wire format exactly. `reply_to` is omitted when absent.
 * `identity` is the top-level UNS identity element (UNS-CANONICAL-DESIGN §1) —
 * optional on the wire: a message built without a config-bound builder (the
 * CONFIG_COMPONENT bootstrap request, raw bridging) legally omits it. `tags` is
 * emitted only when tags were stamped (config-bound builder or explicit
 * `withTags`/`withTag`), matching Java's null-tags omission. The legacy
 * `tags.thing` stamp is removed — hard cut (§1.1): a stray inbound `thing` key
 * just lands in the generic tag map.
 *
 * A message can also be **raw** (a non-envelope payload). A received payload that
 * is not an envelope (none of `header`/`identity`/`tags`/`body`, or not a JSON
 * object) is delivered as a raw message carrying the original value, mirroring
 * Java's `Message.getRaw()`, Python's `Message.raw` and Rust's `Message::get_raw`.
 * A raw message serializes as `{ "raw": <value> }`.
 */
import { randomUUID } from "crypto";

import { logger } from "./logging";

/** Message metadata. Field names are the snake_case wire keys. */
export interface MessageHeader {
  name: string;
  version: string;
  timestamp: string;
  correlation_id: string;
  uuid: string;
  /** Reply-to topic, present only on request messages. */
  reply_to?: string;
}

/** Arbitrary message tags. */
export type MessageTags = Record<string, unknown>;

/** One level of the UNS enterprise hierarchy: the level name and this deployment's value. */
export interface HierLevel {
  level: string;
  value: string;
}

/**
 * The top-level `identity` envelope element of the unified namespace (UNS-CANONICAL-DESIGN §1).
 *
 * One immutable class serves as both the wire object and the component's resolved identity
 * (see `Config.componentIdentity`). It carries:
 * - `hier` — the ordered enterprise hierarchy (length >= 1); its **last entry is always the
 *   physical device**. There is no standalone `device` wire field — {@link device} is a
 *   computed accessor over the last entry.
 * - `path` — the precomputed `'/'`-join of the `hier` values. The publisher is authoritative:
 *   on deserialize a present `path` is taken as-is, a missing one is recomputed.
 * - `component` — the publishing component's UNS token (the sanitized short name, i.e. the
 *   existing `{ComponentName}` semantics).
 * - `instance` — the per-message instance token, never absent (default
 *   {@link MessageIdentity.DEFAULT_INSTANCE}).
 *
 * Serialization ({@link toObject}) emits the canonical member order
 * `hier, path, component, instance`. Deserialization ({@link fromObject}) is deliberately
 * lenient, mirroring the lenient envelope handling across all four libraries: a malformed
 * `identity` yields `undefined` plus a WARN log, and the message still delivers.
 */
export class MessageIdentity {
  /** The default per-message instance token, used when no instance is specified. */
  static readonly DEFAULT_INSTANCE = "main";

  private readonly hierValue: readonly HierLevel[];
  private readonly pathValue: string;
  private readonly componentValue: string;
  private readonly instanceValue: string;

  /**
   * Creates a validated identity, precomputing `path` as the `'/'`-join of the `hier` values.
   *
   * @param hier      the ordered hierarchy entries (non-empty; last entry = device)
   * @param component the component UNS token (non-empty)
   * @param instance  the instance token, or absent/empty for {@link MessageIdentity.DEFAULT_INSTANCE}
   * @param path      wire-authoritative path override (library-internal — used by
   *                  {@link fromObject} where a present wire path is taken as-is)
   * @throws Error if `hier` is empty, an entry has an empty level/value, or `component` is empty
   */
  constructor(hier: readonly HierLevel[], component: string, instance?: string, path?: string) {
    if (!Array.isArray(hier) || hier.length === 0) {
      throw new Error("MessageIdentity hier must contain at least one entry");
    }
    for (const entry of hier) {
      if (typeof entry.level !== "string" || entry.level === "") {
        throw new Error("MessageIdentity hier entry level must be non-empty");
      }
      if (typeof entry.value !== "string" || entry.value === "") {
        throw new Error(`MessageIdentity hier entry value for level '${entry.level}' must be non-empty`);
      }
    }
    if (typeof component !== "string" || component === "") {
      throw new Error("MessageIdentity component must be non-empty");
    }
    this.hierValue = hier.map((e) => ({ level: e.level, value: e.value }));
    this.pathValue = path ?? this.hierValue.map((e) => e.value).join("/");
    this.componentValue = component;
    this.instanceValue = instance === undefined || instance === "" ? MessageIdentity.DEFAULT_INSTANCE : instance;
  }

  /** The immutable, ordered hierarchy entries (the last entry is the device). */
  get hier(): readonly HierLevel[] {
    return this.hierValue;
  }

  /** The precomputed `'/'`-join of the hierarchy values. */
  get path(): string {
    return this.pathValue;
  }

  /** The component UNS token (the sanitized short name). */
  get component(): string {
    return this.componentValue;
  }

  /** The per-message instance token (never absent). */
  get instance(): string {
    return this.instanceValue;
  }

  /**
   * Computed accessor — the last `hier` entry's value. NOT a wire field: the device is
   * inherent to the hierarchy (its deepest level), so it is never serialized separately.
   */
  get device(): string {
    return this.hierValue[this.hierValue.length - 1].value;
  }

  /**
   * Returns a copy of this identity with a different per-message instance token.
   *
   * @throws Error if `instance` is empty
   */
  withInstance(instance: string): MessageIdentity {
    if (typeof instance !== "string" || instance === "") {
      throw new Error("MessageIdentity instance must be non-empty");
    }
    return new MessageIdentity(this.hierValue, this.componentValue, instance, this.pathValue);
  }

  /**
   * Serializes this identity to its wire form, in the canonical member order
   * `hier, path, component, instance`.
   */
  toObject(): Record<string, unknown> {
    return {
      hier: this.hierValue.map((e) => ({ level: e.level, value: e.value })),
      path: this.pathValue,
      component: this.componentValue,
      instance: this.instanceValue,
    };
  }

  /**
   * Lenient wire-form parser: a missing `instance` defaults to
   * {@link MessageIdentity.DEFAULT_INSTANCE}; a missing `path` is recomputed from the hier
   * values (a present one is taken as-is — the publisher is authoritative); a malformed
   * identity (non-object, missing/empty/non-array `hier`, malformed hier entries, or a
   * missing `component`) yields `undefined` plus a WARN log so the enclosing message still
   * delivers.
   */
  static fromObject(src: unknown): MessageIdentity | undefined {
    if (src === null || typeof src !== "object" || Array.isArray(src)) {
      logger.warn("Malformed message identity: 'identity' is not an object; dropping identity");
      return undefined;
    }
    const obj = src as Record<string, unknown>;
    const hierRaw = obj.hier;
    if (!Array.isArray(hierRaw) || hierRaw.length === 0) {
      logger.warn("Malformed message identity: 'hier' missing, not an array, or empty; dropping identity");
      return undefined;
    }
    const hier: HierLevel[] = [];
    for (const entryRaw of hierRaw) {
      if (entryRaw === null || typeof entryRaw !== "object" || Array.isArray(entryRaw)) {
        logger.warn("Malformed message identity: hier entry is not an object; dropping identity");
        return undefined;
      }
      const entry = entryRaw as Record<string, unknown>;
      const level = asNonEmptyString(entry.level);
      const value = asNonEmptyString(entry.value);
      if (level === undefined || value === undefined) {
        logger.warn("Malformed message identity: hier entry missing level/value; dropping identity");
        return undefined;
      }
      hier.push({ level, value });
    }
    const component = asNonEmptyString(obj.component);
    if (component === undefined) {
      logger.warn("Malformed message identity: 'component' missing or empty; dropping identity");
      return undefined;
    }
    const path = asNonEmptyString(obj.path); // undefined -> recomputed
    const instance = asNonEmptyString(obj.instance); // undefined -> DEFAULT_INSTANCE
    return new MessageIdentity(hier, component, instance, path);
  }
}

/** Returns the value as a non-empty string, or `undefined` if absent/non-string/empty. */
function asNonEmptyString(value: unknown): string | undefined {
  return typeof value === "string" && value !== "" ? value : undefined;
}

/**
 * #16: a binary body (`Buffer`/`Uint8Array`) travels as a base64 JSON string — the portable
 * cross-language interim for binary bodies (otherwise `JSON.stringify(Buffer)` emits a non-portable
 * `{ "type": "Buffer", "data": [...] }`). Matches Java `byte[]` -> base64 and Python `bytes` ->
 * base64, scoped to a top-level binary body, pending a first-class binary message type. Non-binary
 * bodies pass through unchanged.
 */
function encodeBody(value: unknown): unknown {
  return value instanceof Uint8Array ? Buffer.from(value).toString("base64") : value;
}

/**
 * A message: either an **envelope** (header + optional identity + optional tags + body) or a
 * **raw** (non-envelope) payload. For a raw message {@link isRaw} is `true` and
 * {@link getRaw} returns the original value; the envelope fields are unset.
 */
export class Message {
  header: MessageHeader;
  /**
   * The UNS identity element (UNS-CANONICAL-DESIGN §1), or `undefined` (a message built without
   * a config-bound builder, or a malformed inbound identity).
   */
  identity?: MessageIdentity;
  /** The message tags, or `undefined` when none were stamped (omitted on the wire, like Java). */
  tags?: MessageTags;
  body: unknown;
  private raw?: unknown;
  private rawSet: boolean;

  private constructor(
    header: MessageHeader,
    tags: MessageTags | undefined,
    body: unknown,
    raw?: unknown,
    rawSet = false,
    identity?: MessageIdentity,
  ) {
    this.header = header;
    this.identity = identity;
    this.tags = tags;
    this.body = body;
    this.raw = raw;
    this.rawSet = rawSet;
  }

  /** Construct an envelope message from its parts. */
  static envelope(
    header: MessageHeader,
    tags: MessageTags | undefined,
    body: unknown,
    identity?: MessageIdentity,
  ): Message {
    return new Message(header, tags, body, undefined, false, identity);
  }

  /** Construct a raw (non-envelope) message carrying `value`. */
  static raw(value: unknown): Message {
    return new Message(emptyHeader(), undefined, null, value, true);
  }

  /** Whether this is a raw (non-envelope) message. */
  isRaw(): boolean {
    return this.rawSet;
  }

  /** The raw payload, or `undefined` for an envelope. */
  getRaw(): unknown {
    return this.rawSet ? this.raw : undefined;
  }

  /** The message body (envelope payload). */
  getBody(): unknown {
    return this.body;
  }

  /**
   * The UNS identity element, or `undefined` (no config-bound builder, or a malformed inbound
   * identity — the lenient parser drops it with a WARN and the message still delivers).
   */
  getIdentity(): MessageIdentity | undefined {
    return this.identity;
  }

  /** Correlation id used to match a reply to its request. */
  getCorrelationId(): string {
    return this.header.correlation_id;
  }

  /** Reply-to topic, if present. */
  getReplyTo(): string | undefined {
    return this.header.reply_to;
  }

  /**
   * The wire object for this message: `{ raw }` for a raw message, else the envelope in the
   * canonical member order `header, identity, tags, body` — `identity` and `tags` are omitted
   * when unset, and `reply_to` is omitted when absent, matching Java/Python/Rust.
   */
  toObject(): Record<string, unknown> {
    if (this.rawSet) {
      return { raw: encodeBody(this.raw) };
    }
    const header: Record<string, unknown> = {
      name: this.header.name,
      version: this.header.version,
      timestamp: this.header.timestamp,
      correlation_id: this.header.correlation_id,
      uuid: this.header.uuid,
    };
    if (this.header.reply_to !== undefined && this.header.reply_to !== "") {
      header.reply_to = this.header.reply_to;
    }
    const out: Record<string, unknown> = { header };
    if (this.identity !== undefined) {
      out.identity = this.identity.toObject();
    }
    if (this.tags !== undefined) {
      out.tags = { ...this.tags };
    }
    out.body = encodeBody(this.body);
    return out;
  }

  /** Serialize this message to a JSON string for the wire. */
  toJSON(): string {
    return JSON.stringify(this.toObject());
  }

  /**
   * Classify a parsed JSON value into an envelope or a raw message. An object carrying any of
   * `header`/`identity`/`tags`/`body` is an envelope (missing parts default; a malformed
   * `identity` is dropped leniently); anything else becomes a raw message — matching the other
   * libs.
   */
  static fromObject(value: unknown): Message {
    if (value !== null && typeof value === "object" && !Array.isArray(value)) {
      const obj = value as Record<string, unknown>;
      const isEnvelope = "header" in obj || "identity" in obj || "tags" in obj || "body" in obj;
      if (isEnvelope) {
        const header = { ...emptyHeader(), ...((obj.header as object) ?? {}) } as MessageHeader;
        const identity = "identity" in obj ? MessageIdentity.fromObject(obj.identity) : undefined;
        const tags = "tags" in obj ? (((obj.tags as MessageTags) ?? {}) as MessageTags) : undefined;
        const body = "body" in obj ? obj.body : null;
        return Message.envelope(header, tags, body, identity);
      }
    }
    return Message.raw(value);
  }

  /**
   * Deserialize a message from a wire payload. Valid JSON is classified into an
   * envelope or a raw message; bytes that are **not valid JSON** are delivered as
   * a raw string, so a message is never silently dropped (matching the other libs).
   */
  static fromWire(payload: Buffer | string): Message {
    const text = typeof payload === "string" ? payload : payload.toString("utf8");
    try {
      return Message.fromObject(JSON.parse(text));
    } catch {
      return Message.raw(text);
    }
  }
}

/** A header with empty/defaulted fields (no fresh ids). */
function emptyHeader(): MessageHeader {
  return { name: "", version: "", timestamp: "", correlation_id: "", uuid: "" };
}

/**
 * Fluent builder for {@link Message}, the supported construction path. `create`
 * stamps a fresh `uuid`, `correlation_id` and ISO-8601 `timestamp` (pin them with
 * {@link withUuid}/{@link withTimestamp}/{@link withCorrelationId} for deterministic
 * envelopes — tests and the cross-language `uns-test-vectors` goldens, D-U13).
 *
 * `build()` is the single UNS identity stamping site (UNS-CANONICAL-DESIGN §1.4): an
 * explicit {@link withIdentity} override wins; otherwise, when a config snapshot is
 * present ({@link withConfig}), the component's resolved identity is stamped with the
 * per-message instance token ({@link withInstance}, default
 * {@link MessageIdentity.DEFAULT_INSTANCE}); with neither, `identity` stays unset
 * (bootstrap/raw messages legally omit it).
 */
export class MessageBuilder {
  private header: MessageHeader;
  private extra: MessageTags = {};
  private tagsPresent = false;
  private bodyValue: unknown = null;
  private instanceToken?: string;
  private identityOverride?: MessageIdentity;
  private configIdentity?: MessageIdentity;

  private constructor(name: string, version: string) {
    this.header = {
      name,
      version,
      timestamp: new Date().toISOString(),
      correlation_id: randomUUID(),
      uuid: randomUUID(),
    };
  }

  /** Start building a message with the given name and version. */
  static create(name: string, version: string): MessageBuilder {
    return new MessageBuilder(name, version);
  }

  /** Set the message body. */
  withPayload(body: unknown): this {
    this.bodyValue = body;
    return this;
  }

  /** Replace the tags. */
  withTags(tags: MessageTags): this {
    this.extra = { ...tags };
    this.tagsPresent = true;
    return this;
  }

  /** Add a single tag. */
  withTag(key: string, value: unknown): this {
    this.extra[key] = value;
    this.tagsPresent = true;
    return this;
  }

  /**
   * Populate the tags and the config-resolved component identity from a config snapshot
   * (mirrors the Java `withConfig(ConfigManager)`). Typed structurally to avoid a config
   * import. `build()` stamps `componentIdentity.withInstance(<instance token>)` unless an
   * explicit {@link withIdentity} override is set.
   */
  withConfig(config: {
    parsed: { tags: Record<string, unknown> };
    componentIdentity?: MessageIdentity;
  }): this {
    this.tagsPresent = true;
    for (const [k, v] of Object.entries(config.parsed.tags)) {
      this.extra[k] = v;
    }
    this.configIdentity = config.componentIdentity;
    return this;
  }

  /** Override the correlation id (e.g. to correlate a reply with its request). */
  withCorrelationId(id: string): this {
    this.header.correlation_id = id;
    return this;
  }

  /**
   * Pin the header `uuid` instead of the generated random one — deterministic envelopes for
   * tests and the cross-language `uns-test-vectors` golden envelopes (D-U13).
   */
  withUuid(uuid: string): this {
    this.header.uuid = uuid;
    return this;
  }

  /**
   * Pin the header `timestamp` instead of the generated "now" — deterministic envelopes for
   * tests and the cross-language `uns-test-vectors` golden envelopes (D-U13).
   */
  withTimestamp(timestamp: string): this {
    this.header.timestamp = timestamp;
    return this;
  }

  /**
   * Set the per-message instance token stamped into the identity element (default
   * {@link MessageIdentity.DEFAULT_INSTANCE}). Only takes effect when an identity is stamped
   * (a config snapshot is present; the token is not applied to an explicit
   * {@link withIdentity} override).
   *
   * @throws Error if `instance` is empty
   */
  withInstance(instance: string): this {
    if (typeof instance !== "string" || instance === "") {
      throw new Error("instance must be non-empty");
    }
    this.instanceToken = instance;
    return this;
  }

  /**
   * Set an explicit identity override (tests, conformance vectors, relays). Wins over the
   * config-resolved identity and is stamped verbatim (the {@link withInstance} token is not
   * applied to an override).
   */
  withIdentity(identity: MessageIdentity): this {
    this.identityOverride = identity;
    return this;
  }

  /** Set the reply-to topic, marking this as a request. */
  withReplyTo(topic: string): this {
    this.header.reply_to = topic;
    return this;
  }

  /** Finalize the message (the single identity stamping site, §1.4). */
  build(): Message {
    // Explicit override > config-resolved component identity (+ per-message instance
    // token) > none (bootstrap/raw cases stay valid).
    let identity: MessageIdentity | undefined;
    if (this.identityOverride !== undefined) {
      identity = this.identityOverride;
    } else if (this.configIdentity !== undefined) {
      identity = this.configIdentity.withInstance(this.instanceToken ?? MessageIdentity.DEFAULT_INSTANCE);
    }
    const tags = this.tagsPresent ? { ...this.extra } : undefined;
    return Message.envelope({ ...this.header }, tags, this.bodyValue, identity);
  }
}
