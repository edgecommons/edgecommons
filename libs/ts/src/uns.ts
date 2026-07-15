/**
 * The unified-namespace (UNS) topic builder + validator (UNS-CANONICAL-DESIGN §2), bound to a
 * {@link MessageIdentity} and the component's `topic.includeRoot` setting. Obtain the
 * component-bound instance via `gg.uns()` (component scope — no instance token, D-U28) or an
 * instance-bound one via `gg.instance(id).uns()`. Mirrors the Java
 * `com.mbreissi.edgecommons.uns` package.
 *
 * Grammar (§2.2): `ecv1 [/ {site}]? / {device} / {component} [/ {instance}]? / {class}
 * [/ {channel…}]` — the `{instance}` slot is optional (D-U28: present ⇒ instance scope, absent ⇒
 * component scope); the optional `site` position (the first hierarchy value) is emitted only
 * when `topic.includeRoot` is `true` **and** the identity carries a multi-level hierarchy
 * (≥ 2 `hier` entries — D-U25). With a single-level hierarchy (`["device"]`) `hier[0]` *is*
 * the device, so includeRoot is a no-op (prepending would duplicate the device).
 *
 * Normative rules enforced here (each violation throws {@link UnsValidationError} with a
 * machine-readable {@link UnsValidationCode}):
 * 1. **Token rule** — identical to the config template sanitizer's blacklist
 *    (`config/template.ts` `sanitize`), so "sanitized ⇒ valid" is a true equivalence (D-U26):
 *    a token is non-empty, contains no `/ + # \`, no ISO control characters (C0
 *    U+0000–U+001F, U+007F, **and C1 U+0080–U+009F**), and no `..` substring. Dots are legal.
 * 2. **Depth guard** — at most {@link MAX_TOPIC_SLASHES} `/` separators total (AWS IoT Core's
 *    8-level limit), so the channel budget is 3 tokens rootless / 2 tokens rooted; enforced at
 *    build time.
 * 3. **Length** — at most {@link MAX_TOPIC_UTF8_BYTES} UTF-8 bytes total.
 * 4. **Class rules** — leaf classes (`state`, `cfg`) forbid a channel; every other class
 *    requires at least one channel token.
 *
 * Reply topics (`edgecommons/reply-…`) are non-UNS and never pass through this builder.
 */
import { MessageIdentity } from "./message";
import { isIsoControl } from "./config/template";

/** The UNS root literal — the first token of every UNS topic. */
export const UNS_ROOT = "ecv1";

/** AWS IoT Core's 8-level topic limit, expressed as the maximum `/` separator count. */
export const MAX_TOPIC_SLASHES = 7;

/** AWS IoT Core's topic publish limit in UTF-8 bytes. */
export const MAX_TOPIC_UTF8_BYTES = 256;

/**
 * The closed UNS class set (§2.1) — the class topic level of every UNS topic. Enum values are
 * the wire tokens exactly as they appear in topics.
 */
export enum UnsClass {
  /** Component liveness/state keepalive (library-owned). Leaf. */
  State = "state",
  /** Component metrics (library-owned). Channeled. */
  Metric = "metric",
  /** Effective-configuration announcements (library-owned). Leaf. */
  Cfg = "cfg",
  /** Log tailing (library-owned; publisher lands in a later phase). Channeled. */
  Log = "log",
  /** Application telemetry/data. Channeled. */
  Data = "data",
  /** Application events. Channeled. */
  Evt = "evt",
  /** Command inboxes (request/reply verbs). Channeled. */
  Cmd = "cmd",
  /** Free-form application namespace. Channeled. */
  App = "app",
}

/** Leaf classes: the class token is the last topic level — a channel is forbidden. */
const LEAF_CLASSES: ReadonlySet<UnsClass> = new Set([UnsClass.State, UnsClass.Cfg]);

/** The library-owned publish classes (`state | metric | cfg | log`) — reserved (§4.1). */
export const RESERVED_CLASSES: ReadonlySet<UnsClass> = new Set([
  UnsClass.State,
  UnsClass.Metric,
  UnsClass.Cfg,
  UnsClass.Log,
]);

/** Leaf semantics: `true` — channel forbidden; `false` — channel REQUIRED. */
export function isLeafClass(cls: UnsClass): boolean {
  return LEAF_CLASSES.has(cls);
}

/**
 * Resolves a wire token to its class, or `undefined` when the token is outside the closed set.
 */
export function unsClassFromToken(token: string): UnsClass | undefined {
  return (Object.values(UnsClass) as string[]).includes(token) ? (token as UnsClass) : undefined;
}

/**
 * The machine-readable UNS validation failure codes (the exact §2.2 set, pinned in
 * `uns-test-vectors/topics.json` so all four languages fail identically).
 */
export type UnsValidationCode =
  | "EMPTY_TOKEN"
  | "BAD_CHAR"
  | "TRAVERSAL"
  | "DEPTH_EXCEEDED"
  | "LENGTH_EXCEEDED"
  | "CHANNEL_ON_LEAF"
  | "CHANNEL_REQUIRED"
  | "BAD_ROOT"
  | "BAD_CLASS"
  | "WILDCARD_IN_TOPIC";

/**
 * Thrown by the {@link Uns} topic builder/validator when a topic, filter component, or token
 * violates the UNS grammar (§2.2). Carries a machine-readable {@link code} so callers (and the
 * cross-language `uns-test-vectors`) can assert the exact failure without parsing the
 * human-readable message.
 */
export class UnsValidationError extends Error {
  /** The machine-readable failure code. */
  readonly code: UnsValidationCode;

  constructor(code: UnsValidationCode, message: string) {
    super(`[${code}] ${message}`);
    this.name = "UnsValidationError";
    this.code = code;
  }
}

/**
 * The wildcard scope for {@link Uns.filter} (§2.1). An absent field renders as the MQTT
 * single-level wildcard `+` at that topic position; a present field pins the position to that
 * concrete token. The `site` field is used only when the bound `topic.includeRoot` is
 * effective (the rooted grammar has a site position between the root and the device); it is
 * ignored otherwise.
 */
export interface UnsScope {
  /** The first-hierarchy-level value to pin (rooted grammar only), or absent for `+`. */
  site?: string;
  /** The device (thing) token to pin, or absent for `+`. */
  device?: string;
  /** The component token to pin, or absent for `+`. */
  component?: string;
  /** The instance token to pin, or absent for `+`. */
  instance?: string;
}

/** Factory helpers for common {@link UnsScope} shapes (mirrors the Java record's statics). */
export const UnsScope = {
  /** Every position wildcarded — all devices, components and instances. */
  all(): UnsScope {
    return {};
  },
  /** All components/instances on one device. */
  device(device: string): UnsScope {
    return { device };
  },
  /** All instances of one component on one device. */
  component(device: string, component: string): UnsScope {
    return { device, component };
  },
  /** One exact instance of one component on one device. */
  instance(device: string, component: string, instance: string): UnsScope {
    return { device, component, instance };
  },
};

/**
 * The §2.2 **token rule** — deliberately the EXACT SAME blacklist as the config template
 * sanitizer (`config/template.ts` `sanitize`), so "sanitized ⇒ valid" is a true equivalence
 * (D-U26): non-empty, no `/ + # \`, no ISO control characters (C0 U+0000–U+001F, U+007F, and
 * C1 U+0080–U+009F), no `..` substring. Also the validation gate for `gg.instance(id)`
 * instance tokens. If anyone later tightens the sanitizer, this rule must tighten with it
 * (and vice versa).
 *
 * @param token the token to check
 * @param what  what the token is, for the error message (e.g. `"instance id"`)
 * @throws UnsValidationError `EMPTY_TOKEN` / `BAD_CHAR` / `TRAVERSAL`
 */
export function checkToken(token: string | undefined | null, what: string): void {
  if (token === undefined || token === null || token === "") {
    throw new UnsValidationError("EMPTY_TOKEN", `${what} must be a non-empty token`);
  }
  for (let i = 0; i < token.length; i++) {
    const c = token[i];
    // D-U26: the sanitizer's control-char predicate (covers C0, DEL, and C1 U+0080-U+009F).
    if (c === "/" || c === "+" || c === "#" || c === "\\" || isIsoControl(token.charCodeAt(i))) {
      throw new UnsValidationError(
        "BAD_CHAR",
        `${what} '${token}' contains a forbidden character at index ${i}` +
          " (no '/', '+', '#', '\\' or ISO control characters)",
      );
    }
  }
  if (token.includes("..")) {
    throw new UnsValidationError("TRAVERSAL", `${what} '${token}' contains the traversal sequence '..'`);
  }
}

/** {@link checkToken} that returns the (valid) token, for inline segment assembly. */
function checkedToken(token: string, what: string): string {
  checkToken(token, what);
  return token;
}

/** Renders a scope field: absent as the `+` wildcard, else the checked token. */
function wildcardOr(value: string | undefined, what: string): string {
  return value === undefined ? "+" : checkedToken(value, what);
}

/** Enforces the {@link MAX_TOPIC_UTF8_BYTES} topic length limit. */
function checkLength(topic: string): void {
  const bytes = Buffer.byteLength(topic, "utf8");
  if (bytes > MAX_TOPIC_UTF8_BYTES) {
    throw new UnsValidationError(
      "LENGTH_EXCEEDED",
      `topic is ${bytes} UTF-8 bytes (max ${MAX_TOPIC_UTF8_BYTES})`,
    );
  }
}

/**
 * The UNS topic builder + validator bound to an identity and a root mode (§2). Library wiring —
 * components obtain bound instances from the `EdgeCommons` facade (`gg.uns()` /
 * `gg.instance(id).uns()`).
 */
export class Uns {
  private readonly identityValue: MessageIdentity;
  private readonly includeRoot: boolean;

  /**
   * @param identity    the identity whose tokens {@link topic} emits
   * @param includeRoot whether topics/filters carry the first hierarchy value (`site`) between
   *                    the {@link UNS_ROOT} root and the device (`topic.includeRoot`, default
   *                    `false`). Effective only for identities with a multi-level hierarchy
   *                    (≥ 2 `hier` entries) — a no-op otherwise (D-U25)
   */
  constructor(identity: MessageIdentity, includeRoot: boolean) {
    this.identityValue = identity;
    this.includeRoot = includeRoot;
  }

  /** Returns the bound identity. */
  identity(): MessageIdentity {
    return this.identityValue;
  }

  /**
   * Builds the bound identity's concrete topic: for a **leaf** class (`state`, `cfg`) omit the
   * channel; for a channeled class supply one or more `/`-separated channel tokens (≤ 3
   * rootless, ≤ 2 rooted), e.g. `"temp"` or `"sb/status"`. An absent/empty channel means "no
   * channel" (only legal for leaf classes).
   *
   * @returns the concrete topic, e.g. `ecv1/gw-01/opcua-adapter/data/temp` (component scope) or
   *          `ecv1/gw-01/opcua-adapter/kep1/data/temp` (instance scope)
   * @throws UnsValidationError on any §2.2 violation
   */
  topic(cls: UnsClass, channel?: string): string {
    return this.topicFor(this.identityValue, cls, channel);
  }

  /**
   * Builds a concrete topic for a **peer's** identity — typically a received message's
   * `getIdentity()` — which is how a component addresses a peer's `cmd` inbox without parsing
   * topics. The target's tokens pass the same token rule as the bound identity's.
   *
   * @throws UnsValidationError on any §2.2 violation
   */
  topicFor(target: MessageIdentity, cls: UnsClass, channel?: string): string {
    // D-U25: the site position exists only for a multi-level hierarchy — with a single-level
    // hierarchy hier[0] IS the device, so prepending it would duplicate the device level.
    const rooted = this.rooted(target);
    const segments: string[] = [UNS_ROOT];
    if (rooted) {
      segments.push(checkedToken(target.hier[0].value, "site (hier[0]) value"));
    }
    segments.push(checkedToken(target.device, "device"));
    segments.push(checkedToken(target.component, "component"));
    if (target.instance !== undefined) {
      // D-U28: the instance slot is omitted entirely for component scope.
      segments.push(checkedToken(target.instance, "instance"));
    }
    segments.push(cls);

    const channelSupplied = channel !== undefined && channel !== "";
    if (isLeafClass(cls) && channelSupplied) {
      throw new UnsValidationError(
        "CHANNEL_ON_LEAF",
        `class '${cls}' is a leaf class - a channel is forbidden (got '${channel}')`,
      );
    }
    if (!isLeafClass(cls) && !channelSupplied) {
      throw new UnsValidationError(
        "CHANNEL_REQUIRED",
        `class '${cls}' requires at least one channel token`,
      );
    }
    if (channelSupplied) {
      for (const channelToken of channel.split("/")) {
        segments.push(checkedToken(channelToken, "channel token"));
      }
    }

    const topic = segments.join("/");
    const slashes = segments.length - 1;
    if (slashes > MAX_TOPIC_SLASHES) {
      throw new UnsValidationError(
        "DEPTH_EXCEEDED",
        `topic '${topic}' has ${slashes} '/' separators (max ${MAX_TOPIC_SLASHES};` +
          ` the channel budget is ${rooted ? 2 : 3} token(s) with an effective root mode of ${rooted})`,
      );
    }
    checkLength(topic);
    return topic;
  }

  /**
   * Builds a subscription filter for a class over a wildcard {@link UnsScope}: absent scope
   * fields render as `+`; channeled classes get a trailing `/#` (all channels); leaf classes
   * end at the class token. The `site` position exists (and `scope.site` is consulted) only
   * when `topic.includeRoot` is `true` AND the bound identity carries a multi-level hierarchy
   * (D-U25).
   *
   * The output is correct by construction and is NOT passed through {@link validate} (filters
   * legitimately carry wildcards).
   *
   * D-U28 scope-aware: when `includeInstance` is `false` the instance slot is **omitted**
   * entirely, yielding a component-scope filter (e.g. `ecv1/{device}/{component}/cmd/#`); when
   * `true` (the default) the instance slot is present (a pinned token, or `+` when
   * `scope.instance` is absent).
   *
   * @param cls             the UNS class
   * @param scope           the wildcard scope
   * @param includeInstance whether to render the optional instance slot (default `true`)
   * @returns the subscription filter, e.g. `ecv1/+/+/+/data/#`
   * @throws UnsValidationError when a pinned scope field violates the token rule
   */
  filter(cls: UnsClass, scope: UnsScope, includeInstance = true): string {
    const segments: string[] = [UNS_ROOT];
    if (this.rooted(this.identityValue)) {
      segments.push(wildcardOr(scope.site, "site"));
    }
    segments.push(wildcardOr(scope.device, "device"));
    segments.push(wildcardOr(scope.component, "component"));
    if (includeInstance) {
      segments.push(wildcardOr(scope.instance, "instance"));
    }
    segments.push(cls);
    const filter = segments.join("/");
    return isLeafClass(cls) ? filter : `${filter}/#`;
  }

  /**
   * Validates a **concrete** topic against the full §2.2 grammar under this instance's root
   * mode: wildcards are rejected (`WILDCARD_IN_TOPIC`); every token passes the token rule; the
   * first token must be the {@link UNS_ROOT} root literal; depth ≤ {@link MAX_TOPIC_SLASHES}
   * separators; length ≤ {@link MAX_TOPIC_UTF8_BYTES} UTF-8 bytes; the class is **located** by
   * the class-token set (D-U28: the instance slot is optional, so the token after `{component}`
   * is the class when it is a class token, else the instance and the class follows) and must
   * hold a {@link UnsClass} token; leaf classes must end at the class token and channeled classes
   * must carry at least one channel token. The minimum length is 4 tokens without an instance / 5
   * with one (one more when rooted; the root mode is effective only with a multi-level bound
   * hierarchy, D-U25).
   *
   * @throws UnsValidationError with the precise code on the first violation found
   */
  validate(topic: string): void {
    if (topic === undefined || topic === null || topic === "") {
      throw new UnsValidationError("EMPTY_TOKEN", "topic is null or empty");
    }
    if (topic.includes("+") || topic.includes("#")) {
      throw new UnsValidationError(
        "WILDCARD_IN_TOPIC",
        `validate() accepts only concrete topics - '${topic}' contains an MQTT wildcard ('+'/'#')`,
      );
    }
    const tokens = topic.split("/");
    for (const token of tokens) {
      checkToken(token, "topic token");
    }
    if (tokens[0] !== UNS_ROOT) {
      throw new UnsValidationError(
        "BAD_ROOT",
        `topic '${topic}' must start with the UNS root '${UNS_ROOT}' (got '${tokens[0]}')`,
      );
    }
    const slashes = tokens.length - 1;
    if (slashes > MAX_TOPIC_SLASHES) {
      throw new UnsValidationError(
        "DEPTH_EXCEEDED",
        `topic '${topic}' has ${slashes} '/' separators (max ${MAX_TOPIC_SLASHES})`,
      );
    }
    checkLength(topic);
    // D-U28: the instance slot is optional. The token right after {component} is the class iff it
    // is a class token, else it is the instance and the class follows (an instance is never a
    // class token). {component} sits at index 2 rootless / 3 rooted.
    const rooted = this.rooted(this.identityValue);
    const afterComponent = rooted ? 4 : 3;
    if (tokens.length <= afterComponent) {
      throw new UnsValidationError(
        "BAD_CLASS",
        `topic '${topic}' has too few levels (${tokens.length}): no class token at or after` +
          ` position ${afterComponent} (effective root mode ${rooted})`,
      );
    }
    const classPosition = unsClassFromToken(tokens[afterComponent]) !== undefined ? afterComponent : afterComponent + 1;
    if (tokens.length <= classPosition) {
      throw new UnsValidationError(
        "BAD_CLASS",
        `topic '${topic}' carries an instance token but no class token follows` +
          ` (expected at position ${classPosition})`,
      );
    }
    const cls = unsClassFromToken(tokens[classPosition]);
    if (cls === undefined) {
      throw new UnsValidationError(
        "BAD_CLASS",
        `'${tokens[classPosition]}' (position ${classPosition} of '${topic}') is not a UNS class token`,
      );
    }
    const hasChannel = tokens.length > classPosition + 1;
    if (isLeafClass(cls) && hasChannel) {
      throw new UnsValidationError(
        "CHANNEL_ON_LEAF",
        `class '${cls}' is a leaf class - topic '${topic}' must end at the class token`,
      );
    }
    if (!isLeafClass(cls) && !hasChannel) {
      throw new UnsValidationError(
        "CHANNEL_REQUIRED",
        `class '${cls}' requires at least one channel token - topic '${topic}' ends at the class token`,
      );
    }
  }

  /**
   * The effective root mode for an identity (D-U25): `topic.includeRoot` applies only when the
   * identity carries a multi-level hierarchy — with a single-level hierarchy `hier[0]` *is*
   * the device, so the site position does not exist and includeRoot is a no-op (the config
   * model WARNs once at config time).
   */
  private rooted(target: MessageIdentity): boolean {
    return this.includeRoot && target.hier.length >= 2;
  }
}

/**
 * The §4.1 reserved-class guard predicate (D-U24): the reserved class a client-chosen topic
 * targets, or `undefined` when the topic is allowed. The class position is topic level 4
 * (0-based) always — the rootless grammar `ecv1/{device}/{component}/{instance}/{class}` — and
 * level 5 **only when this component's effective `topic.includeRoot` is true** (D-U27: bind
 * `includeRoot && hier.length >= 2`, the same effective-root rule topic-building uses).
 * Non-`ecv1` topics pass untouched (`edgecommons/reply-…`, `cloudwatch/metric/put`, foreign MQTT
 * bridging).
 */
export function reservedClassOf(topic: string | undefined, includeRoot: boolean): UnsClass | undefined {
  if (topic === undefined || topic === null || !topic.startsWith(UNS_ROOT)) {
    return undefined;
  }
  const tokens = topic.split("/");
  if (tokens[0] !== UNS_ROOT) {
    return undefined;
  }
  if (tokens.length >= 5) {
    const cls = unsClassFromToken(tokens[4]);
    if (cls !== undefined && RESERVED_CLASSES.has(cls)) {
      return cls;
    }
  }
  if (includeRoot && tokens.length >= 6) {
    const cls = unsClassFromToken(tokens[5]);
    if (cls !== undefined && RESERVED_CLASSES.has(cls)) {
      return cls;
    }
  }
  return undefined;
}
