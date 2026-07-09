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
import { createHash, randomUUID } from "crypto";

import { logger } from "./logging";

export const MAX_BINARY_BODY_BYTES = 64 * 1024;
const BINARY_BODY_KEY = "_edgecommonsBinary";
const BINARY_ENCODING = "base64";
const DEFAULT_OPAQUE_CONTENT_TYPE = "application/octet-stream";

export enum MessageBodyCase {
  SouthboundSignalUpdate = "SOUTHBOUND_SIGNAL_UPDATE",
  StateUpdate = "STATE_UPDATE",
  ConfigUpdate = "CONFIG_UPDATE",
  MetricUpdate = "METRIC_UPDATE",
  Event = "EVENT",
  Command = "COMMAND",
  Structured = "STRUCTURED",
  Opaque = "OPAQUE",
  BodyNotSet = "BODY_NOT_SET",
}

export interface MessageBodySchema {
  name?: string;
  version?: string;
  content_type?: string;
  descriptor_ref?: string;
  hash?: string;
}

/** Message metadata. Field names are the snake_case wire keys. */
export interface MessageHeader {
  name: string;
  version: string;
  timestamp: string;
  timestamp_ms?: number;
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
 * Encode a top-level binary body (`Buffer`/`Uint8Array`) as the first-class bounded binary marker.
 */
function encodeBody(value: unknown): unknown {
  if (!(value instanceof Uint8Array)) {
    return value;
  }
  const bytes = Buffer.from(value);
  if (bytes.length > MAX_BINARY_BODY_BYTES) {
    throw new Error(`Binary message body exceeds ${MAX_BINARY_BODY_BYTES} bytes`);
  }
  return {
    [BINARY_BODY_KEY]: {
      encoding: BINARY_ENCODING,
      length: bytes.length,
      data: bytes.toString("base64"),
    },
  };
}

function binaryDescriptor(value: unknown): Record<string, unknown> | undefined {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    return undefined;
  }
  const obj = value as Record<string, unknown>;
  if (!(BINARY_BODY_KEY in obj)) {
    return undefined;
  }
  const descriptor = obj[BINARY_BODY_KEY];
  if (descriptor === null || typeof descriptor !== "object" || Array.isArray(descriptor)) {
    throw new Error("Binary message body marker must be an object");
  }
  return descriptor as Record<string, unknown>;
}

function decodeBinaryDescriptor(descriptor: Record<string, unknown>): Buffer {
  if (descriptor.encoding !== BINARY_ENCODING) {
    throw new Error("Binary message body encoding must be base64");
  }
  if (!Number.isInteger(descriptor.length) || (descriptor.length as number) < 0) {
    throw new Error("Binary message body length must be a non-negative integer");
  }
  const declaredLength = descriptor.length as number;
  if (declaredLength > MAX_BINARY_BODY_BYTES) {
    throw new Error(`Binary message body exceeds ${MAX_BINARY_BODY_BYTES} bytes`);
  }
  if (typeof descriptor.data !== "string" || !isStrictBase64(descriptor.data)) {
    throw new Error("Binary message body data is not valid base64");
  }
  const decoded = Buffer.from(descriptor.data, "base64");
  if (decoded.length !== declaredLength) {
    throw new Error("Binary message body length does not match decoded data");
  }
  return decoded;
}

function isStrictBase64(value: string): boolean {
  if (value === "") {
    return true;
  }
  return /^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(value);
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
  contentType?: string;
  contentEncoding?: string;
  schema?: MessageBodySchema;
  bodyCase?: MessageBodyCase;
  private raw?: unknown;
  private rawSet: boolean;

  private constructor(
    header: MessageHeader,
    tags: MessageTags | undefined,
    body: unknown,
    raw?: unknown,
    rawSet = false,
    identity?: MessageIdentity,
    contentType?: string,
    contentEncoding?: string,
    schema?: MessageBodySchema,
    bodyCase?: MessageBodyCase,
  ) {
    this.header = header;
    this.identity = identity;
    this.tags = tags;
    this.body = body;
    this.contentType = contentType;
    this.contentEncoding = contentEncoding;
    this.schema = schema;
    this.bodyCase = bodyCase;
    this.raw = raw;
    this.rawSet = rawSet;
  }

  /** Construct an envelope message from its parts. */
  static envelope(
    header: MessageHeader,
    tags: MessageTags | undefined,
    body: unknown,
    identity?: MessageIdentity,
    metadata?: {
      contentType?: string;
      contentEncoding?: string;
      schema?: MessageBodySchema;
      bodyCase?: MessageBodyCase;
    },
  ): Message {
    return new Message(
      header,
      tags,
      body,
      undefined,
      false,
      identity,
      metadata?.contentType,
      metadata?.contentEncoding,
      metadata?.schema,
      metadata?.bodyCase,
    );
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

  /** Whether the payload is a first-class binary body. */
  isBinaryBody(): boolean {
    return (
      this.body instanceof Uint8Array ||
      (this.body !== null && typeof this.body === "object" && !Array.isArray(this.body) && BINARY_BODY_KEY in this.body)
    );
  }

  /**
   * Decode the first-class binary message body.
   *
   * @returns the decoded bytes, or `undefined` when the body is not binary
   * @throws Error when the inbound binary marker is malformed or too large
   */
  getBinaryBody(): Buffer | undefined {
    if (this.body instanceof Uint8Array) {
      const bytes = Buffer.from(this.body);
      if (bytes.length > MAX_BINARY_BODY_BYTES) {
        throw new Error(`Binary message body exceeds ${MAX_BINARY_BODY_BYTES} bytes`);
      }
      return bytes;
    }
    const descriptor = binaryDescriptor(this.body);
    return descriptor === undefined ? undefined : decodeBinaryDescriptor(descriptor);
  }

  getOpaqueBody(): Buffer | undefined {
    return this.getBodyCase() === MessageBodyCase.Opaque ? this.getBinaryBody() : undefined;
  }

  getContentType(): string | undefined {
    return this.contentType;
  }

  getContentEncoding(): string | undefined {
    return this.contentEncoding;
  }

  getSchema(): MessageBodySchema | undefined {
    return this.schema;
  }

  getBodyCase(): MessageBodyCase {
    return this.bodyCase ?? inferBodyCase(this);
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
      timestamp_ms: this.header.timestamp_ms ?? timestampMsFromString(this.header.timestamp),
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
    if (this.contentType !== undefined) {
      out.content_type = this.contentType;
    }
    if (this.contentEncoding !== undefined) {
      out.content_encoding = this.contentEncoding;
    }
    if (this.schema !== undefined) {
      out.schema = { ...this.schema };
    }
    out.body = encodeBody(this.body);
    return out;
  }

  /** Serialize this message to a JSON string for the wire. */
  toJSON(): string {
    return JSON.stringify(this.toObject());
  }

  toBytes(): Buffer {
    return encodeMessage(this);
  }

  toDiagnosticJson(): Record<string, unknown> {
    const diagnostic = Message.fromBytes(this.toBytes()).toObject();
    diagnostic.body_case = this.getBodyCase();
    if (this.getBodyCase() === MessageBodyCase.Opaque) {
      const bytes = this.getBinaryBody() ?? Buffer.alloc(0);
      diagnostic.body = {
        content_type: this.contentType ?? DEFAULT_OPAQUE_CONTENT_TYPE,
        length: bytes.length,
        sha256: sha256Hex(bytes),
      };
    }
    return diagnostic;
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
        return Message.envelope(header, tags, body, identity, {
          contentType: typeof obj.content_type === "string" ? obj.content_type : undefined,
          contentEncoding: typeof obj.content_encoding === "string" ? obj.content_encoding : undefined,
          schema:
            obj.schema !== null && typeof obj.schema === "object" && !Array.isArray(obj.schema)
              ? (obj.schema as MessageBodySchema)
              : undefined,
        });
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

  static fromBytes(payload: Buffer | Uint8Array): Message {
    return decodeMessage(Buffer.from(payload));
  }
}

/** A header with empty/defaulted fields (no fresh ids). */
function emptyHeader(): MessageHeader {
  return { name: "", version: "", timestamp: "", timestamp_ms: 0, correlation_id: "", uuid: "" };
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
  private contentType?: string;
  private contentEncoding?: string;
  private schema?: MessageBodySchema;
  private bodyCase?: MessageBodyCase;
  private instanceToken?: string;
  private identityOverride?: MessageIdentity;
  private configIdentity?: MessageIdentity;

  private constructor(name: string, version: string) {
    this.header = {
      name,
      version,
      timestamp: new Date().toISOString(),
      timestamp_ms: Date.now(),
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
    if (body instanceof Uint8Array && this.bodyCase === undefined) {
      this.bodyCase = MessageBodyCase.Opaque;
      this.contentType ??= DEFAULT_OPAQUE_CONTENT_TYPE;
    }
    return this;
  }

  withStructuredPayload(body: unknown): this {
    this.bodyValue = body;
    this.bodyCase = MessageBodyCase.Structured;
    return this;
  }

  withStructuredBody(body: unknown): this {
    return this.withStructuredPayload(body);
  }

  withSouthboundSignalUpdate(body: unknown): this {
    this.bodyValue = body;
    this.bodyCase = MessageBodyCase.SouthboundSignalUpdate;
    return this;
  }

  withStateUpdate(body: unknown): this {
    this.bodyValue = body;
    this.bodyCase = MessageBodyCase.StateUpdate;
    return this;
  }

  withConfigUpdate(body: unknown): this {
    this.bodyValue = body;
    this.bodyCase = MessageBodyCase.ConfigUpdate;
    return this;
  }

  withMetricUpdate(body: unknown): this {
    this.bodyValue = body;
    this.bodyCase = MessageBodyCase.MetricUpdate;
    return this;
  }

  withEvent(body: unknown): this {
    this.bodyValue = body;
    this.bodyCase = MessageBodyCase.Event;
    return this;
  }

  withCommand(body: unknown): this {
    this.bodyValue = body;
    this.bodyCase = MessageBodyCase.Command;
    return this;
  }

  withOpaquePayload(body: Uint8Array, contentType = DEFAULT_OPAQUE_CONTENT_TYPE): this {
    this.bodyValue = Buffer.from(body);
    this.bodyCase = MessageBodyCase.Opaque;
    this.contentType = contentType;
    return this;
  }

  withOpaqueBody(body: Uint8Array, contentType = DEFAULT_OPAQUE_CONTENT_TYPE): this {
    return this.withOpaquePayload(body, contentType);
  }

  withContentType(contentType: string): this {
    this.contentType = contentType;
    return this;
  }

  withContentEncoding(contentEncoding: string): this {
    this.contentEncoding = contentEncoding;
    return this;
  }

  withSchema(schema: MessageBodySchema): this {
    this.schema = { ...schema };
    return this;
  }

  withBodyCase(bodyCase: MessageBodyCase): this {
    this.bodyCase = bodyCase;
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
    this.header.timestamp_ms = timestampMsFromString(timestamp);
    return this;
  }

  withTimestampMs(timestampMs: number): this {
    this.header.timestamp_ms = timestampMs;
    this.header.timestamp = new Date(timestampMs).toISOString();
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
    return Message.envelope({ ...this.header }, tags, this.bodyValue, identity, {
      contentType: this.contentType,
      contentEncoding: this.contentEncoding,
      schema: this.schema,
      bodyCase: this.bodyCase,
    });
  }
}

function timestampMsFromString(timestamp: string): number {
  const parsed = Date.parse(timestamp);
  return Number.isFinite(parsed) ? parsed : 0;
}

function inferBodyCase(message: Message): MessageBodyCase {
  if (message.getBody() === null || message.getBody() === undefined) {
    return MessageBodyCase.BodyNotSet;
  }
  if (message.isBinaryBody()) {
    return MessageBodyCase.Opaque;
  }
  if ((message.header.name === "SouthboundSignalUpdate" || message.header.name === "Telemetry") && isPlainObject(message.getBody())) {
    return MessageBodyCase.SouthboundSignalUpdate;
  }
  if (isPlainObject(message.getBody())) {
    if (message.header.name.toLowerCase() === "state") return MessageBodyCase.StateUpdate;
    if (message.header.name.toLowerCase() === "cfg" || message.header.name === "Config" || message.header.name === "Configuration") return MessageBodyCase.ConfigUpdate;
    if (message.header.name === "Metric" || message.header.name === "metric") return MessageBodyCase.MetricUpdate;
    if (message.header.name.toLowerCase() === "evt" || message.header.name === "Event") return MessageBodyCase.Event;
  }
  return MessageBodyCase.Structured;
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value) && !(value instanceof Uint8Array);
}

function sha256Hex(bytes: Buffer): string {
  return createHash("sha256").update(bytes).digest("hex");
}

const WT_VARINT = 0;
const WT_64 = 1;
const WT_LEN = 2;
const WT_32 = 5;

class ProtoWriter {
  private readonly chunks: Buffer[] = [];

  tag(field: number, wireType: number): this {
    return this.uint32((field << 3) | wireType);
  }

  uint32(value: number): this {
    return this.varint(BigInt(value >>> 0));
  }

  uint64(value: number | bigint): this {
    const n = typeof value === "bigint" ? value : BigInt(Math.trunc(value));
    return this.varint(n < 0n ? BigInt.asUintN(64, n) : n);
  }

  int64(value: number | bigint): this {
    return this.uint64(value);
  }

  bool(value: boolean): this {
    return this.uint32(value ? 1 : 0);
  }

  double(value: number): this {
    const buf = Buffer.allocUnsafe(8);
    buf.writeDoubleLE(value, 0);
    this.chunks.push(buf);
    return this;
  }

  string(value: string): this {
    const buf = Buffer.from(value, "utf8");
    this.uint32(buf.length);
    this.chunks.push(buf);
    return this;
  }

  bytes(value: Buffer | Uint8Array): this {
    const buf = Buffer.from(value);
    this.uint32(buf.length);
    this.chunks.push(buf);
    return this;
  }

  message(field: number, encode: () => Buffer): this {
    const buf = encode();
    this.tag(field, WT_LEN).bytes(buf);
    return this;
  }

  finish(): Buffer {
    return Buffer.concat(this.chunks);
  }

  private varint(value: bigint): this {
    let n = value;
    const bytes: number[] = [];
    do {
      let b = Number(n & 0x7fn);
      n >>= 7n;
      if (n !== 0n) b |= 0x80;
      bytes.push(b);
    } while (n !== 0n);
    this.chunks.push(Buffer.from(bytes));
    return this;
  }
}

class ProtoReader {
  pos = 0;

  constructor(readonly buf: Buffer) {}

  eof(): boolean {
    return this.pos >= this.buf.length;
  }

  tag(): { field: number; wireType: number } {
    const key = this.uint32();
    return { field: key >>> 3, wireType: key & 7 };
  }

  uint32(): number {
    return Number(this.varint());
  }

  uint64(): number {
    return Number(this.varint());
  }

  int64(): number {
    const n = this.varint();
    const signed = n & (1n << 63n) ? n - (1n << 64n) : n;
    return Number(signed);
  }

  bool(): boolean {
    return this.uint32() !== 0;
  }

  double(): number {
    this.need(8);
    const v = this.buf.readDoubleLE(this.pos);
    this.pos += 8;
    return v;
  }

  string(): string {
    return this.bytes().toString("utf8");
  }

  bytes(): Buffer {
    const len = this.uint32();
    this.need(len);
    const out = this.buf.subarray(this.pos, this.pos + len);
    this.pos += len;
    return Buffer.from(out);
  }

  subReader(): ProtoReader {
    return new ProtoReader(this.bytes());
  }

  skip(wireType: number): void {
    switch (wireType) {
      case WT_VARINT:
        this.varint();
        return;
      case WT_64:
        this.need(8);
        this.pos += 8;
        return;
      case WT_LEN: {
        const len = this.uint32();
        this.need(len);
        this.pos += len;
        return;
      }
      case WT_32:
        this.need(4);
        this.pos += 4;
        return;
      default:
        throw new Error("Malformed EdgeCommons protobuf message");
    }
  }

  private varint(): bigint {
    let shift = 0n;
    let result = 0n;
    for (let i = 0; i < 10; i++) {
      this.need(1);
      const b = this.buf[this.pos++];
      result |= BigInt(b & 0x7f) << shift;
      if ((b & 0x80) === 0) return result;
      shift += 7n;
    }
    throw new Error("Malformed EdgeCommons protobuf message");
  }

  private need(n: number): void {
    if (this.pos + n > this.buf.length) {
      throw new Error("Malformed EdgeCommons protobuf message");
    }
  }
}

function encodeMessage(message: Message): Buffer {
  if (message.isRaw()) {
    throw new Error("EdgeCommons protobuf message requires a header");
  }
  if (!message.header.name || !message.header.version) {
    throw new Error("EdgeCommons protobuf message requires header name and version");
  }
  const w = new ProtoWriter();
  w.message(1, () => encodeHeader(message.header));
  if (message.identity !== undefined) w.message(2, () => encodeIdentity(message.identity!));
  if (message.tags !== undefined) {
    for (const key of Object.keys(message.tags).sort()) {
      w.message(3, () => encodeStringValueEntry(key, encodeEcValue(message.tags![key])));
    }
  }
  const bodyCase = message.getBodyCase();
  if (bodyCase === MessageBodyCase.Opaque) {
    w.tag(4, WT_LEN).string(message.contentType ?? DEFAULT_OPAQUE_CONTENT_TYPE);
  } else if (message.contentType !== undefined) {
    w.tag(4, WT_LEN).string(message.contentType);
  }
  if (message.contentEncoding !== undefined) w.tag(5, WT_LEN).string(message.contentEncoding);
  if (message.schema !== undefined) w.message(6, () => encodeSchema(message.schema!));

  switch (bodyCase) {
    case MessageBodyCase.SouthboundSignalUpdate:
      w.message(20, () => encodeSouthboundSignalUpdate(asRecord(message.body)));
      break;
    case MessageBodyCase.StateUpdate:
      w.message(21, () => encodeStateUpdate(asRecord(message.body)));
      break;
    case MessageBodyCase.ConfigUpdate:
      w.message(22, () => encodeConfigUpdate(asRecord(message.body)));
      break;
    case MessageBodyCase.MetricUpdate:
      w.message(23, () => encodeMetricUpdate(asRecord(message.body)));
      break;
    case MessageBodyCase.Event:
      w.message(24, () => encodeEventMessage(asRecord(message.body)));
      break;
    case MessageBodyCase.Command:
      w.message(25, () => encodeCommandMessage(message.header.name, asRecord(message.body)));
      break;
    case MessageBodyCase.Structured:
      w.message(30, () => encodeEcValue(message.body));
      break;
    case MessageBodyCase.Opaque:
      w.tag(31, WT_LEN).bytes(message.getBinaryBody() ?? Buffer.alloc(0));
      break;
    case MessageBodyCase.BodyNotSet:
      break;
  }
  return w.finish();
}

function decodeMessage(buf: Buffer): Message {
  try {
    const r = new ProtoReader(buf);
    let header: MessageHeader | undefined;
    let identity: MessageIdentity | undefined;
    const tags: MessageTags = {};
    let tagsPresent = false;
    let contentType: string | undefined;
    let contentEncoding: string | undefined;
    let schema: MessageBodySchema | undefined;
    let body: unknown = null;
    let bodyCase = MessageBodyCase.BodyNotSet;

    while (!r.eof()) {
      const { field, wireType } = r.tag();
      switch (field) {
        case 1:
          header = decodeHeader(r.subReader());
          break;
        case 2:
          identity = decodeIdentity(r.subReader());
          break;
        case 3: {
          const [key, value] = decodeStringValueEntry(r.subReader());
          tags[key] = value;
          tagsPresent = true;
          break;
        }
        case 4:
          contentType = r.string();
          break;
        case 5:
          contentEncoding = r.string();
          break;
        case 6:
          schema = decodeSchema(r.subReader());
          break;
        case 20:
          body = decodeSouthboundSignalUpdate(r.subReader());
          bodyCase = MessageBodyCase.SouthboundSignalUpdate;
          break;
        case 21:
          body = decodeStateUpdate(r.subReader());
          bodyCase = MessageBodyCase.StateUpdate;
          break;
        case 22:
          body = decodeConfigUpdate(r.subReader());
          bodyCase = MessageBodyCase.ConfigUpdate;
          break;
        case 23:
          body = decodeMetricUpdate(r.subReader());
          bodyCase = MessageBodyCase.MetricUpdate;
          break;
        case 24:
          body = decodeEventMessage(r.subReader());
          bodyCase = MessageBodyCase.Event;
          break;
        case 25:
          body = decodeCommandMessage(r.subReader());
          bodyCase = MessageBodyCase.Command;
          break;
        case 30:
          body = decodeEcValue(r.subReader());
          bodyCase = MessageBodyCase.Structured;
          break;
        case 31:
          body = Buffer.from(r.bytes());
          contentType ??= DEFAULT_OPAQUE_CONTENT_TYPE;
          bodyCase = MessageBodyCase.Opaque;
          break;
        default:
          r.skip(wireType);
      }
    }
    if (header === undefined || !header.name || !header.version) {
      throw new Error("EdgeCommons protobuf message requires header name and version");
    }
    return Message.envelope(header, tagsPresent ? tags : undefined, body, identity, {
      contentType,
      contentEncoding,
      schema,
      bodyCase,
    });
  } catch (e) {
    if (e instanceof Error && e.message.startsWith("EdgeCommons protobuf")) throw e;
    throw new Error("Malformed EdgeCommons protobuf message");
  }
}

function encodeHeader(header: MessageHeader): Buffer {
  const w = new ProtoWriter();
  w.tag(1, WT_LEN).string(header.name);
  w.tag(2, WT_LEN).string(header.version);
  w.tag(3, WT_VARINT).uint64(header.timestamp_ms ?? timestampMsFromString(header.timestamp));
  w.tag(4, WT_LEN).string(header.uuid);
  if (header.correlation_id !== undefined) w.tag(5, WT_LEN).string(header.correlation_id);
  if (header.reply_to !== undefined) w.tag(6, WT_LEN).string(header.reply_to);
  return w.finish();
}

function decodeHeader(r: ProtoReader): MessageHeader {
  const header = emptyHeader();
  while (!r.eof()) {
    const { field, wireType } = r.tag();
    switch (field) {
      case 1:
        header.name = r.string();
        break;
      case 2:
        header.version = r.string();
        break;
      case 3:
        header.timestamp_ms = r.uint64();
        header.timestamp = new Date(header.timestamp_ms).toISOString();
        break;
      case 4:
        header.uuid = r.string();
        break;
      case 5:
        header.correlation_id = r.string();
        break;
      case 6:
        header.reply_to = r.string();
        break;
      default:
        r.skip(wireType);
    }
  }
  return header;
}

function encodeIdentity(identity: MessageIdentity): Buffer {
  const w = new ProtoWriter();
  for (const entry of identity.hier) w.message(1, () => encodeHierEntry(entry));
  w.tag(2, WT_LEN).string(identity.path);
  w.tag(3, WT_LEN).string(identity.component);
  w.tag(4, WT_LEN).string(identity.instance);
  return w.finish();
}

function decodeIdentity(r: ProtoReader): MessageIdentity | undefined {
  const hier: HierLevel[] = [];
  let path = "";
  let component = "";
  let instance = "";
  while (!r.eof()) {
    const { field, wireType } = r.tag();
    switch (field) {
      case 1:
        hier.push(decodeHierEntry(r.subReader()));
        break;
      case 2:
        path = r.string();
        break;
      case 3:
        component = r.string();
        break;
      case 4:
        instance = r.string();
        break;
      default:
        r.skip(wireType);
    }
  }
  return MessageIdentity.fromObject({ hier, path, component, instance });
}

function encodeHierEntry(entry: HierLevel): Buffer {
  const w = new ProtoWriter();
  w.tag(1, WT_LEN).string(entry.level);
  w.tag(2, WT_LEN).string(entry.value);
  return w.finish();
}

function decodeHierEntry(r: ProtoReader): HierLevel {
  const entry = { level: "", value: "" };
  while (!r.eof()) {
    const { field, wireType } = r.tag();
    if (field === 1) entry.level = r.string();
    else if (field === 2) entry.value = r.string();
    else r.skip(wireType);
  }
  return entry;
}

function encodeSchema(schema: MessageBodySchema): Buffer {
  const w = new ProtoWriter();
  if (schema.name !== undefined) w.tag(1, WT_LEN).string(schema.name);
  if (schema.version !== undefined) w.tag(2, WT_LEN).string(schema.version);
  if (schema.content_type !== undefined) w.tag(3, WT_LEN).string(schema.content_type);
  if (schema.descriptor_ref !== undefined) w.tag(4, WT_LEN).string(schema.descriptor_ref);
  if (schema.hash !== undefined) w.tag(5, WT_LEN).string(schema.hash);
  return w.finish();
}

function decodeSchema(r: ProtoReader): MessageBodySchema {
  const schema: MessageBodySchema = {};
  while (!r.eof()) {
    const { field, wireType } = r.tag();
    if (field === 1) schema.name = r.string();
    else if (field === 2) schema.version = r.string();
    else if (field === 3) schema.content_type = r.string();
    else if (field === 4) schema.descriptor_ref = r.string();
    else if (field === 5) schema.hash = r.string();
    else r.skip(wireType);
  }
  return schema;
}

function encodeSouthboundSignalUpdate(body: Record<string, unknown>): Buffer {
  const w = new ProtoWriter();
  if (isPlainObject(body.signal)) w.message(1, () => encodeSignal(body.signal as Record<string, unknown>));
  if (Array.isArray(body.samples)) {
    for (const sample of body.samples) {
      if (isPlainObject(sample)) w.message(2, () => encodeSample(sample as Record<string, unknown>));
    }
  }
  encodeExtra(w, body, 100, ["signal", "samples"]);
  return w.finish();
}

function decodeSouthboundSignalUpdate(r: ProtoReader): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  const samples: unknown[] = [];
  while (!r.eof()) {
    const { field, wireType } = r.tag();
    if (field === 1) out.signal = decodeSignal(r.subReader());
    else if (field === 2) samples.push(decodeSample(r.subReader()));
    else if (field === 100) {
      const [key, value] = decodeStringValueEntry(r.subReader());
      out[key] = value;
    } else r.skip(wireType);
  }
  out.samples = samples;
  return out;
}

function encodeSignal(signal: Record<string, unknown>): Buffer {
  const w = new ProtoWriter();
  if (typeof signal.id === "string") w.tag(1, WT_LEN).string(signal.id);
  if (typeof signal.name === "string") w.tag(2, WT_LEN).string(signal.name);
  if ("address" in signal) w.message(3, () => encodeEcValue(signal.address));
  encodeExtra(w, signal, 100, ["id", "name", "address"]);
  return w.finish();
}

function decodeSignal(r: ProtoReader): Record<string, unknown> {
  const signal: Record<string, unknown> = {};
  while (!r.eof()) {
    const { field, wireType } = r.tag();
    if (field === 1) signal.id = r.string();
    else if (field === 2) signal.name = r.string();
    else if (field === 3) signal.address = decodeEcValue(r.subReader());
    else if (field === 100) {
      const [key, value] = decodeStringValueEntry(r.subReader());
      signal[key] = value;
    } else r.skip(wireType);
  }
  return signal;
}

function encodeSample(sample: Record<string, unknown>): Buffer {
  const w = new ProtoWriter();
  if ("value" in sample) w.message(1, () => encodeEcValue(sample.value));
  if (typeof sample.quality === "string") w.tag(2, WT_LEN).string(sample.quality);
  if ("qualityRaw" in sample) w.message(3, () => encodeEcValue(sample.qualityRaw));
  else if ("quality_raw" in sample) w.message(3, () => encodeEcValue(sample.quality_raw));
  const sourceTs = stringValue(sample.sourceTs) ?? stringValue(sample.source_ts);
  if (sourceTs !== undefined) {
    w.tag(4, WT_LEN).string(sourceTs);
    const parsed = timestampMsFromString(sourceTs);
    if (parsed !== 0 && !("sourceTsMs" in sample) && !("source_ts_ms" in sample)) w.tag(5, WT_VARINT).uint64(parsed);
  }
  const sourceTsMs = numberValue(sample.sourceTsMs) ?? numberValue(sample.source_ts_ms);
  if (sourceTsMs !== undefined) w.tag(5, WT_VARINT).uint64(sourceTsMs);
  const serverTs = stringValue(sample.serverTs) ?? stringValue(sample.server_ts);
  if (serverTs !== undefined) {
    w.tag(6, WT_LEN).string(serverTs);
    const parsed = timestampMsFromString(serverTs);
    if (parsed !== 0 && !("serverTsMs" in sample) && !("server_ts_ms" in sample)) w.tag(7, WT_VARINT).uint64(parsed);
  }
  const serverTsMs = numberValue(sample.serverTsMs) ?? numberValue(sample.server_ts_ms);
  if (serverTsMs !== undefined) w.tag(7, WT_VARINT).uint64(serverTsMs);
  encodeExtra(w, sample, 100, [
    "value",
    "quality",
    "qualityRaw",
    "quality_raw",
    "sourceTs",
    "source_ts",
    "sourceTsMs",
    "source_ts_ms",
    "serverTs",
    "server_ts",
    "serverTsMs",
    "server_ts_ms",
  ]);
  return w.finish();
}

function decodeSample(r: ProtoReader): Record<string, unknown> {
  const sample: Record<string, unknown> = {};
  while (!r.eof()) {
    const { field, wireType } = r.tag();
    if (field === 1) sample.value = decodeEcValue(r.subReader());
    else if (field === 2) sample.quality = r.string();
    else if (field === 3) sample.qualityRaw = decodeEcValue(r.subReader());
    else if (field === 4) sample.sourceTs = r.string();
    else if (field === 5) sample.sourceTsMs = r.uint64();
    else if (field === 6) sample.serverTs = r.string();
    else if (field === 7) sample.serverTsMs = r.uint64();
    else if (field === 100) {
      const [key, value] = decodeStringValueEntry(r.subReader());
      sample[key] = value;
    } else r.skip(wireType);
  }
  return sample;
}

function encodeStateUpdate(body: Record<string, unknown>): Buffer {
  const w = new ProtoWriter();
  const status = stringValue(body.status);
  if (status !== undefined) w.tag(1, WT_LEN).string(status);
  const uptimeSecs = numberValue(body.uptimeSecs) ?? numberValue(body.uptime_secs);
  if (uptimeSecs !== undefined) w.tag(2, WT_VARINT).uint64(uptimeSecs);
  if (Array.isArray(body.instances)) {
    for (const item of body.instances) {
      if (isPlainObject(item)) w.message(3, () => encodeInstanceConnectivity(item));
    }
  }
  encodeExtra(w, body, 100, ["status", "uptimeSecs", "uptime_secs", "instances"]);
  return w.finish();
}

function decodeStateUpdate(r: ProtoReader): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  const instances: unknown[] = [];
  while (!r.eof()) {
    const { field, wireType } = r.tag();
    if (field === 1) out.status = r.string();
    else if (field === 2) out.uptimeSecs = r.uint64();
    else if (field === 3) instances.push(decodeInstanceConnectivity(r.subReader()));
    else if (field === 100) {
      const [key, value] = decodeStringValueEntry(r.subReader());
      out[key] = value;
    } else r.skip(wireType);
  }
  if (instances.length > 0) out.instances = instances;
  return out;
}

function encodeInstanceConnectivity(item: Record<string, unknown>): Buffer {
  const w = new ProtoWriter();
  const instance = stringValue(item.instance);
  if (instance !== undefined) w.tag(1, WT_LEN).string(instance);
  if (typeof item.connected === "boolean") w.tag(2, WT_VARINT).bool(item.connected);
  const detail = stringValue(item.detail);
  if (detail !== undefined) w.tag(3, WT_LEN).string(detail);
  encodeExtra(w, item, 100, ["instance", "connected", "detail"]);
  return w.finish();
}

function decodeInstanceConnectivity(r: ProtoReader): Record<string, unknown> {
  const out: Record<string, unknown> = { connected: false };
  while (!r.eof()) {
    const { field, wireType } = r.tag();
    if (field === 1) out.instance = r.string();
    else if (field === 2) out.connected = r.bool();
    else if (field === 3) out.detail = r.string();
    else if (field === 100) {
      const [key, value] = decodeStringValueEntry(r.subReader());
      out[key] = value;
    } else r.skip(wireType);
  }
  return out;
}

function encodeConfigUpdate(body: Record<string, unknown>): Buffer {
  const w = new ProtoWriter();
  if ("config" in body) w.message(1, () => encodeEcValue(body.config));
  else w.message(1, () => encodeEcValue(body));
  encodeExtra(w, body, 100, ["config"]);
  return w.finish();
}

function decodeConfigUpdate(r: ProtoReader): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  while (!r.eof()) {
    const { field, wireType } = r.tag();
    if (field === 1) out.config = decodeEcValue(r.subReader());
    else if (field === 100) {
      const [key, value] = decodeStringValueEntry(r.subReader());
      out[key] = value;
    } else r.skip(wireType);
  }
  return out;
}

function encodeMetricUpdate(body: Record<string, unknown>): Buffer {
  const w = new ProtoWriter();
  const namespace = stringValue(body.namespace);
  if (namespace !== undefined) w.tag(1, WT_LEN).string(namespace);
  const metricName = stringValue(body.metricName) ?? stringValue(body.metric_name);
  if (metricName !== undefined) w.tag(2, WT_LEN).string(metricName);
  const timestampMs = numberValue(body.timestampMs) ?? numberValue(body.timestamp_ms);
  if (timestampMs !== undefined) w.tag(3, WT_VARINT).uint64(timestampMs);
  if (isPlainObject(body.dimensions)) {
    for (const key of Object.keys(body.dimensions).sort()) {
      w.message(4, () => encodeStringStringEntry(key, String((body.dimensions as Record<string, unknown>)[key])));
    }
  }
  if (Array.isArray(body.values)) {
    for (const item of body.values) {
      if (isPlainObject(item)) w.message(5, () => encodeMetricValue(item));
    }
  }
  if (typeof body.largeFleetWorkaround === "boolean") w.tag(6, WT_VARINT).bool(body.largeFleetWorkaround);
  else if (typeof body.large_fleet_workaround === "boolean") w.tag(6, WT_VARINT).bool(body.large_fleet_workaround);
  if ("emfProjection" in body) w.message(20, () => encodeEcValue(body.emfProjection));
  else if ("emf_projection" in body) w.message(20, () => encodeEcValue(body.emf_projection));
  encodeExtra(w, body, 100, [
    "namespace",
    "metricName",
    "metric_name",
    "timestampMs",
    "timestamp_ms",
    "dimensions",
    "values",
    "largeFleetWorkaround",
    "large_fleet_workaround",
    "emfProjection",
    "emf_projection",
  ]);
  return w.finish();
}

function decodeMetricUpdate(r: ProtoReader): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  const dimensions: Record<string, string> = {};
  const values: unknown[] = [];
  while (!r.eof()) {
    const { field, wireType } = r.tag();
    if (field === 1) out.namespace = r.string();
    else if (field === 2) out.metricName = r.string();
    else if (field === 3) out.timestampMs = r.uint64();
    else if (field === 4) {
      const [key, value] = decodeStringStringEntry(r.subReader());
      dimensions[key] = value;
    } else if (field === 5) values.push(decodeMetricValue(r.subReader()));
    else if (field === 6) out.largeFleetWorkaround = r.bool();
    else if (field === 20) out.emfProjection = decodeEcValue(r.subReader());
    else if (field === 100) {
      const [key, value] = decodeStringValueEntry(r.subReader());
      out[key] = value;
    } else r.skip(wireType);
  }
  if (Object.keys(dimensions).length > 0) out.dimensions = dimensions;
  if (values.length > 0) out.values = values;
  return out;
}

function encodeMetricValue(value: Record<string, unknown>): Buffer {
  const w = new ProtoWriter();
  const name = stringValue(value.name);
  if (name !== undefined) w.tag(1, WT_LEN).string(name);
  const numeric = numberValue(value.value);
  if (numeric !== undefined) w.tag(2, WT_64).double(numeric);
  const unit = stringValue(value.unit);
  if (unit !== undefined) w.tag(3, WT_LEN).string(unit);
  const storageResolution = numberValue(value.storageResolution) ?? numberValue(value.storage_resolution);
  if (storageResolution !== undefined) w.tag(4, WT_VARINT).uint32(storageResolution);
  return w.finish();
}

function decodeMetricValue(r: ProtoReader): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  while (!r.eof()) {
    const { field, wireType } = r.tag();
    if (field === 1) out.name = r.string();
    else if (field === 2) out.value = r.double();
    else if (field === 3) out.unit = r.string();
    else if (field === 4) out.storageResolution = r.uint32();
    else r.skip(wireType);
  }
  return out;
}

function encodeEventMessage(body: Record<string, unknown>): Buffer {
  const w = new ProtoWriter();
  const severity = stringValue(body.severity);
  if (severity !== undefined) w.tag(1, WT_LEN).string(severity);
  const type = stringValue(body.type);
  if (type !== undefined) w.tag(2, WT_LEN).string(type);
  const message = stringValue(body.message);
  if (message !== undefined) w.tag(3, WT_LEN).string(message);
  const timestamp = stringValue(body.timestamp);
  if (timestamp !== undefined) w.tag(4, WT_LEN).string(timestamp);
  const timestampMs = numberValue(body.timestampMs) ?? numberValue(body.timestamp_ms);
  if (timestampMs !== undefined) w.tag(5, WT_VARINT).uint64(timestampMs);
  if ("context" in body) w.message(6, () => encodeEcValue(body.context));
  if (typeof body.alarm === "boolean") w.tag(7, WT_VARINT).bool(body.alarm);
  if (typeof body.active === "boolean") w.tag(8, WT_VARINT).bool(body.active);
  encodeExtra(w, body, 100, ["severity", "type", "message", "timestamp", "timestampMs", "timestamp_ms", "context", "alarm", "active"]);
  return w.finish();
}

function decodeEventMessage(r: ProtoReader): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  while (!r.eof()) {
    const { field, wireType } = r.tag();
    if (field === 1) out.severity = r.string();
    else if (field === 2) out.type = r.string();
    else if (field === 3) out.message = r.string();
    else if (field === 4) out.timestamp = r.string();
    else if (field === 5) out.timestampMs = r.uint64();
    else if (field === 6) out.context = decodeEcValue(r.subReader());
    else if (field === 7) out.alarm = r.bool();
    else if (field === 8) out.active = r.bool();
    else if (field === 100) {
      const [key, value] = decodeStringValueEntry(r.subReader());
      out[key] = value;
    } else r.skip(wireType);
  }
  return out;
}

function encodeCommandMessage(headerName: string, body: Record<string, unknown>): Buffer {
  const w = new ProtoWriter();
  const verb = stringValue(body.verb) ?? headerName;
  w.tag(1, WT_LEN).string(verb);
  let wrappedPayload = false;
  if ("payload" in body) w.message(2, () => encodeEcValue(body.payload));
  else if (!("ok" in body) && !("result" in body) && !("error" in body)) {
    w.message(2, () => encodeEcValue(body));
    wrappedPayload = true;
  }
  if (typeof body.ok === "boolean") w.tag(3, WT_VARINT).bool(body.ok);
  if ("result" in body) w.message(4, () => encodeEcValue(body.result));
  if (isPlainObject(body.error)) w.message(5, () => encodeCommandError(body.error as Record<string, unknown>));
  if (!wrappedPayload) encodeExtra(w, body, 100, ["verb", "payload", "ok", "result", "error"]);
  return w.finish();
}

function decodeCommandMessage(r: ProtoReader): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  let payload: unknown;
  let hasPayload = false;
  let hasOk = false;
  let hasResult = false;
  let hasError = false;
  const extraKeys: string[] = [];
  while (!r.eof()) {
    const { field, wireType } = r.tag();
    if (field === 1) out.verb = r.string();
    else if (field === 2) {
      payload = decodeEcValue(r.subReader());
      hasPayload = true;
    } else if (field === 3) {
      out.ok = r.bool();
      hasOk = true;
    } else if (field === 4) {
      out.result = decodeEcValue(r.subReader());
      hasResult = true;
    } else if (field === 5) {
      out.error = decodeCommandError(r.subReader());
      hasError = true;
    } else if (field === 100) {
      const [key, value] = decodeStringValueEntry(r.subReader());
      out[key] = value;
      extraKeys.push(key);
    } else r.skip(wireType);
  }
  if (hasPayload && !hasOk && !hasResult && !hasError && extraKeys.length === 0) {
    return isPlainObject(payload) ? payload : {};
  }
  if (hasPayload) out.payload = payload;
  return out;
}

function encodeCommandError(error: Record<string, unknown>): Buffer {
  const w = new ProtoWriter();
  const code = stringValue(error.code);
  if (code !== undefined) w.tag(1, WT_LEN).string(code);
  const message = stringValue(error.message);
  if (message !== undefined) w.tag(2, WT_LEN).string(message);
  if (isPlainObject(error.details)) {
    for (const key of Object.keys(error.details).sort()) {
      w.message(100, () => encodeStringValueEntry(key, encodeEcValue((error.details as Record<string, unknown>)[key])));
    }
  }
  return w.finish();
}

function decodeCommandError(r: ProtoReader): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  const details: Record<string, unknown> = {};
  while (!r.eof()) {
    const { field, wireType } = r.tag();
    if (field === 1) out.code = r.string();
    else if (field === 2) out.message = r.string();
    else if (field === 100) {
      const [key, value] = decodeStringValueEntry(r.subReader());
      details[key] = value;
    } else r.skip(wireType);
  }
  if (Object.keys(details).length > 0) out.details = details;
  return out;
}

function encodeEcValue(value: unknown): Buffer {
  const w = new ProtoWriter();
  if (value === null || value === undefined) {
    w.tag(1, WT_VARINT).uint32(0);
  } else if (typeof value === "boolean") {
    w.tag(2, WT_VARINT).bool(value);
  } else if (typeof value === "number") {
    if (!Number.isFinite(value)) throw new Error("EdgeCommons protobuf structured values reject NaN and infinity");
    if (Number.isInteger(value)) w.tag(3, WT_VARINT).int64(value);
    else w.tag(5, WT_64).double(value);
  } else if (typeof value === "bigint") {
    if (value >= 0n) w.tag(4, WT_VARINT).uint64(value);
    else w.tag(3, WT_VARINT).int64(value);
  } else if (typeof value === "string") {
    w.tag(6, WT_LEN).string(value);
  } else if (value instanceof Uint8Array) {
    w.tag(7, WT_LEN).bytes(value);
  } else if (Array.isArray(value)) {
    w.message(8, () => encodeEcList(value));
  } else if (isPlainObject(value)) {
    const binary = decodeBinaryObjectIfPresent(value);
    if (binary !== undefined) w.tag(7, WT_LEN).bytes(binary);
    else w.message(9, () => encodeEcMap(value));
  } else {
    w.tag(6, WT_LEN).string(String(value));
  }
  return w.finish();
}

function decodeEcValue(r: ProtoReader): unknown {
  let value: unknown = null;
  while (!r.eof()) {
    const { field, wireType } = r.tag();
    if (field === 1) {
      r.uint32();
      value = null;
    } else if (field === 2) value = r.bool();
    else if (field === 3) value = r.int64();
    else if (field === 4) value = r.uint64();
    else if (field === 5) value = r.double();
    else if (field === 6) value = r.string();
    else if (field === 7) value = binaryMarker(r.bytes());
    else if (field === 8) value = decodeEcList(r.subReader());
    else if (field === 9) value = decodeEcMap(r.subReader());
    else r.skip(wireType);
  }
  return value;
}

function encodeEcList(values: unknown[]): Buffer {
  const w = new ProtoWriter();
  for (const value of values) w.message(1, () => encodeEcValue(value));
  return w.finish();
}

function decodeEcList(r: ProtoReader): unknown[] {
  const values: unknown[] = [];
  while (!r.eof()) {
    const { field, wireType } = r.tag();
    if (field === 1) values.push(decodeEcValue(r.subReader()));
    else r.skip(wireType);
  }
  return values;
}

function encodeEcMap(obj: Record<string, unknown>): Buffer {
  const w = new ProtoWriter();
  for (const key of Object.keys(obj).sort()) w.message(1, () => encodeStringValueEntry(key, encodeEcValue(obj[key])));
  return w.finish();
}

function decodeEcMap(r: ProtoReader): Record<string, unknown> {
  const obj: Record<string, unknown> = {};
  while (!r.eof()) {
    const { field, wireType } = r.tag();
    if (field === 1) {
      const [key, value] = decodeStringValueEntry(r.subReader());
      obj[key] = value;
    } else r.skip(wireType);
  }
  return obj;
}

function encodeStringValueEntry(key: string, valueBytes: Buffer): Buffer {
  const w = new ProtoWriter();
  w.tag(1, WT_LEN).string(key);
  w.tag(2, WT_LEN).bytes(valueBytes);
  return w.finish();
}

function decodeStringValueEntry(r: ProtoReader): [string, unknown] {
  let key = "";
  let value: unknown = null;
  while (!r.eof()) {
    const { field, wireType } = r.tag();
    if (field === 1) key = r.string();
    else if (field === 2) value = decodeEcValue(r.subReader());
    else r.skip(wireType);
  }
  return [key, value];
}

function encodeStringStringEntry(key: string, value: string): Buffer {
  const w = new ProtoWriter();
  w.tag(1, WT_LEN).string(key);
  w.tag(2, WT_LEN).string(value);
  return w.finish();
}

function decodeStringStringEntry(r: ProtoReader): [string, string] {
  let key = "";
  let value = "";
  while (!r.eof()) {
    const { field, wireType } = r.tag();
    if (field === 1) key = r.string();
    else if (field === 2) value = r.string();
    else r.skip(wireType);
  }
  return [key, value];
}

function encodeExtra(w: ProtoWriter, obj: Record<string, unknown>, field: number, known: string[]): void {
  const knownSet = new Set(known);
  for (const key of Object.keys(obj).sort()) {
    if (!knownSet.has(key)) w.message(field, () => encodeStringValueEntry(key, encodeEcValue(obj[key])));
  }
}

function binaryMarker(bytes: Buffer): Record<string, unknown> {
  return {
    [BINARY_BODY_KEY]: {
      encoding: BINARY_ENCODING,
      length: bytes.length,
      data: bytes.toString("base64"),
    },
  };
}

function decodeBinaryObjectIfPresent(value: Record<string, unknown>): Buffer | undefined {
  const descriptor = binaryDescriptor(value);
  return descriptor === undefined ? undefined : decodeBinaryDescriptor(descriptor);
}

function asRecord(value: unknown): Record<string, unknown> {
  return isPlainObject(value) ? value : {};
}

function stringValue(value: unknown): string | undefined {
  return typeof value === "string" ? value : undefined;
}

function numberValue(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}
