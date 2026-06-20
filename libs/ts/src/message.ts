/**
 * Messaging — Message model (TypeScript).
 *
 * A `Message` is the unit exchanged over any transport. Its JSON shape is kept
 * byte-compatible with the Java, Python and Rust libraries so all four
 * implementations interoperate on the same topics:
 *
 * ```json
 * { "header": { "name", "version", "timestamp", "correlation_id", "uuid", "reply_to" },
 *   "tags":   { "thing": "<thingName>", "...": "..." },
 *   "body":   <any JSON> }
 * ```
 *
 * Header keys are **snake_case** (`correlation_id`, `reply_to`) to match the
 * other libraries' wire format exactly. `reply_to` is omitted when absent, and
 * `thing` is omitted from `tags` when there is no thing name — both to stay
 * byte-identical with Java/Python/Rust.
 *
 * A message can also be **raw** (a non-envelope payload). A received payload that
 * is not an envelope (none of `header`/`tags`/`body`, or not a JSON object) is
 * delivered as a raw message carrying the original value, mirroring Java's
 * `Message.getRaw()`, Python's `Message.raw` and Rust's `Message::get_raw`. A raw
 * message serializes as `{ "raw": <value> }`.
 */
import { randomUUID } from "crypto";

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

/** Arbitrary message tags (plus the thing name, serialized as `thing`). */
export type MessageTags = Record<string, unknown>;

/**
 * A message: either an **envelope** (header + tags + body) or a **raw**
 * (non-envelope) payload. For a raw message {@link isRaw} is `true` and
 * {@link getRaw} returns the original value; the envelope fields are unset.
 */
export class Message {
  header: MessageHeader;
  tags: MessageTags;
  body: unknown;
  private raw?: unknown;
  private rawSet: boolean;

  private constructor(
    header: MessageHeader,
    tags: MessageTags,
    body: unknown,
    raw?: unknown,
    rawSet = false,
  ) {
    this.header = header;
    this.tags = tags;
    this.body = body;
    this.raw = raw;
    this.rawSet = rawSet;
  }

  /** Construct an envelope message from its parts. */
  static envelope(header: MessageHeader, tags: MessageTags, body: unknown): Message {
    return new Message(header, tags, body);
  }

  /** Construct a raw (non-envelope) message carrying `value`. */
  static raw(value: unknown): Message {
    return new Message(emptyHeader(), {}, null, value, true);
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

  /** Correlation id used to match a reply to its request. */
  getCorrelationId(): string {
    return this.header.correlation_id;
  }

  /** Reply-to topic, if present. */
  getReplyTo(): string | undefined {
    return this.header.reply_to;
  }

  /**
   * The wire object for this message: `{ raw }` for a raw message, else the
   * `{ header, tags, body }` envelope. `reply_to` and an empty `thing` are
   * omitted, matching Java/Python/Rust.
   */
  toObject(): Record<string, unknown> {
    if (this.rawSet) {
      return { raw: this.raw };
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
    return { header, tags: { ...this.tags }, body: this.body };
  }

  /** Serialize this message to a JSON string for the wire. */
  toJSON(): string {
    return JSON.stringify(this.toObject());
  }

  /**
   * Classify a parsed JSON value into an envelope or a raw message. An object
   * carrying any of `header`/`tags`/`body` is an envelope (missing parts
   * default); anything else becomes a raw message — matching the other libs.
   */
  static fromObject(value: unknown): Message {
    if (value !== null && typeof value === "object" && !Array.isArray(value)) {
      const obj = value as Record<string, unknown>;
      const isEnvelope = "header" in obj || "tags" in obj || "body" in obj;
      if (isEnvelope) {
        const header = { ...emptyHeader(), ...((obj.header as object) ?? {}) } as MessageHeader;
        const tags = ((obj.tags as MessageTags) ?? {}) as MessageTags;
        const body = "body" in obj ? obj.body : null;
        return Message.envelope(header, tags, body);
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
 * stamps a fresh `uuid`, `correlation_id` and ISO-8601 `timestamp`.
 */
export class MessageBuilder {
  private header: MessageHeader;
  private thingName = "";
  private extra: MessageTags = {};
  private bodyValue: unknown = null;

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

  /** Replace the tags (the thing name, if any, is kept). */
  withTags(tags: MessageTags): this {
    this.extra = { ...tags };
    return this;
  }

  /** Add a single tag. */
  withTag(key: string, value: unknown): this {
    this.extra[key] = value;
    return this;
  }

  /** Set the thing name carried in the tags (serialized as `thing`). */
  withThingName(thing: string): this {
    this.thingName = thing;
    return this;
  }

  /** Override the correlation id (e.g. to correlate a reply with its request). */
  withCorrelationId(id: string): this {
    this.header.correlation_id = id;
    return this;
  }

  /** Set the reply-to topic, marking this as a request. */
  withReplyTo(topic: string): this {
    this.header.reply_to = topic;
    return this;
  }

  /** Finalize the message. */
  build(): Message {
    const tags: MessageTags = { ...this.extra };
    if (this.thingName !== "") {
      tags.thing = this.thingName;
    }
    return Message.envelope({ ...this.header }, tags, this.bodyValue);
  }
}
