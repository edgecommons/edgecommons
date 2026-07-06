/**
 * Configuration — typed model + the runtime {@link Config} snapshot.
 *
 * Mirrors the cross-language JSON schema (`logging`, `heartbeat`, `metricEmission`,
 * `tags`, `component`, `hierarchy`/`identity`/`topic`, `messaging`). {@link Config}
 * pairs the typed view with the original JSON document, the resolved component/thing
 * identity, and the component's resolved **UNS identity** (UNS-CANONICAL-DESIGN §1.5
 * — resolved once, fail-fast). It is immutable; on hot reload a new snapshot
 * replaces the old one atomically.
 *
 * Numeric fields accept a JSON float (Greengrass delivers config numbers as
 * doubles, e.g. `5.0`), matching the Rust `de_lenient_opt_u64` behavior.
 */
import { logger } from "../logging";
import { MessageIdentity } from "../message";
import type { HierLevel } from "../message";
import { sanitize } from "./template";

/** Read a value as an integer, accepting an integer or a (truncated) float. */
function asInt(value: unknown): number | undefined {
  if (typeof value === "number" && Number.isFinite(value)) return Math.trunc(value);
  return undefined;
}

function obj(value: unknown): Record<string, unknown> {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {};
}

/** `logging.fileLogging` section. */
export class FileLoggingConfig {
  enabled: boolean;
  filePath?: string;
  private readonly rawMaxFileSize?: string;
  private readonly rawBackupCount?: number;

  constructor(raw: Record<string, unknown>) {
    this.enabled = raw.enabled === true;
    this.filePath = typeof raw.filePath === "string" ? raw.filePath : undefined;
    this.rawMaxFileSize = typeof raw.maxFileSize === "string" ? raw.maxFileSize : undefined;
    this.rawBackupCount = asInt(raw.backupCount);
  }

  /** `maxFileSize` for size-based rotation; default `10MB`. */
  maxFileSize(): string {
    return this.rawMaxFileSize ?? "10MB";
  }

  /** Number of rotated backups to keep; default `5`. */
  backupCount(): number {
    return this.rawBackupCount ?? 5;
  }
}

/** `logging` section. */
export class LoggingConfig {
  level?: string;
  /** TS log format using {timestamp} {level} {logger} {message} tokens (key `ts_format`). */
  tsFormat?: string;
  fileLogging?: FileLoggingConfig;
  loggers: Record<string, string>;
  globalControl: boolean;

  constructor(raw: Record<string, unknown>) {
    this.level = typeof raw.level === "string" ? raw.level : undefined;
    this.tsFormat = typeof raw.ts_format === "string" ? raw.ts_format : undefined;
    this.fileLogging = raw.fileLogging ? new FileLoggingConfig(obj(raw.fileLogging)) : undefined;
    this.loggers = {};
    for (const [k, v] of Object.entries(obj(raw.loggers))) {
      if (typeof v === "string") this.loggers[k] = v;
    }
    this.globalControl = raw.globalControl === true;
  }
}

/** `heartbeat.measures` toggles. */
export interface Measures {
  cpu: boolean;
  memory: boolean;
  disk: boolean;
  threads: boolean;
  files: boolean;
  fds: boolean;
}

function parseMeasures(raw: Record<string, unknown>): Measures {
  // cpu/memory default ON (the schema defaults, matching Java); the rest default off.
  return {
    cpu: raw.cpu === undefined ? true : raw.cpu === true,
    memory: raw.memory === undefined ? true : raw.memory === true,
    disk: raw.disk === true,
    threads: raw.threads === true,
    files: raw.files === true,
    fds: raw.fds === true,
  };
}

/**
 * `heartbeat` section (UNS-CANONICAL-DESIGN §4.3, D-U14/D-U20).
 *
 * The heartbeat is a library-owned UNS `state` keepalive published each tick to
 * `ecv1/{device}/{component}/main/state` (body `{"status":"RUNNING","uptimeSecs":n}`,
 * best-effort `{"status":"STOPPED"}` on graceful shutdown), with the enabled system measures
 * emitted as the metric `sys` through the normal metric subsystem. The legacy `targets[]`
 * array (the heartbeat topic-override drift knobs) is removed — hard cut; {@link destination}
 * governs only the state keepalive's transport (`local` vs `iotcore`). Defaults: on / 5 s /
 * local (D-U14).
 */
export class HeartbeatConfig {
  /** Whether the heartbeat (state keepalive + `sys` measures metric) runs. Default `true`. */
  enabled: boolean;
  /** Tick interval in seconds; default 5, minimum 1 (out-of-range values fall back to 5). */
  intervalSecs: number;
  measures: Measures;
  /**
   * The publish destination of the `state` keepalive only — `"local"` (the local/IPC
   * transport, the default) or `"iotcore"` (AWS IoT Core). The measures route through the
   * metric subsystem's own target and are unaffected.
   */
  destination: string;

  constructor(raw: Record<string, unknown>) {
    this.enabled = raw.enabled === undefined ? true : raw.enabled === true;
    const interval = asInt(raw.intervalSecs);
    this.intervalSecs = interval !== undefined && interval >= 1 ? interval : 5;
    this.measures = parseMeasures(obj(raw.measures));
    this.destination = typeof raw.destination === "string" ? raw.destination : "local";
  }
}

/** `metricEmission` section, with the same defaulting accessors as the Rust lib. */
export class MetricConfig {
  target_?: string;
  namespace_?: string;
  largeFleetWorkaround: boolean;
  targetConfig?: Record<string, unknown>;

  constructor(raw: Record<string, unknown>) {
    this.target_ = typeof raw.target === "string" ? raw.target : undefined;
    this.namespace_ = typeof raw.namespace === "string" ? raw.namespace : undefined;
    this.largeFleetWorkaround = raw.largeFleetWorkaround === true;
    this.targetConfig = raw.targetConfig !== undefined ? obj(raw.targetConfig) : undefined;
  }

  private targetConfigStr(key: string): string | undefined {
    const v = this.targetConfig?.[key];
    return typeof v === "string" ? v : undefined;
  }

  /**
   * The **explicitly configured** target, or `undefined` when `metricEmission.target` is absent.
   * Exposed so the effective-target precedence (explicit config ▸ platform-profile default ▸ `log`)
   * can be applied by the metrics service without conflating "unset" with the library default.
   */
  explicitTarget(): string | undefined {
    return this.target_;
  }

  /** Selected target (`log`|`messaging`|`cloudwatch`|`cloudwatchcomponent`|`prometheus`); default `log`. */
  target(): string {
    return this.target_ ?? "log";
  }

  /** CloudWatch namespace; default `edgecommons`. */
  namespace(): string {
    return this.namespace_ ?? "edgecommons";
  }

  /** `targetConfig.logFileName` template (log target); default Greengrass path. */
  logFileName(): string {
    return this.targetConfigStr("logFileName") ?? "/greengrass/v2/logs/{ComponentFullName}.metric.log";
  }

  /**
   * The explicit `targetConfig.logFileName` exactly as configured, or `undefined` when absent. Lets the
   * metrics service distinguish an explicit path (which must win) from an unset one (which falls through
   * to the platform-profile default, then the library default) — the HOST-aware metric-log-path
   * precedence. Mirrors {@link explicitTarget}.
   */
  explicitLogFileName(): string | undefined {
    return this.targetConfigStr("logFileName");
  }

  /** `targetConfig.maxFileSize` (log target); default `10MB`. */
  maxFileSize(): string {
    return this.targetConfigStr("maxFileSize") ?? "10MB";
  }

  /** `targetConfig.destination` (messaging target): `ipc`/`local` or `iotcore`; default `ipc`. */
  destination(): string {
    return this.targetConfigStr("destination") ?? "ipc";
  }

  /** `targetConfig.intervalSecs` (cloudwatch batch flush); default 5, minimum 1. */
  intervalSecs(): number {
    const n = asInt(this.targetConfig?.intervalSecs);
    return n !== undefined && n >= 1 ? n : 5;
  }

  /** `targetConfig.port` — the prometheus target's HTTP port (bound `0.0.0.0`); default `9090`. */
  prometheusPort(): number {
    const n = asInt(this.targetConfig?.port);
    return n !== undefined && n >= 1 && n <= 65535 ? n : 9090;
  }

  /** `targetConfig.path` — the prometheus target's OpenMetrics exposition path; default `/metrics`. */
  prometheusPath(): string {
    return this.targetConfigStr("path") ?? "/metrics";
  }

  /**
   * The `cloudwatch` target's optional durable-buffer settings (`targetConfig.buffer`, per the
   * canonical schema). When `type` is `durable` (the default), the cloudwatch target
   * stores-and-forwards metrics through a disk-backed edgestreamlog buffer; `memory` selects the
   * legacy in-memory batching target. Returns `undefined` only if no `buffer` object is present
   * (caller then defaults to durable).
   */
  cloudwatchBuffer(): CloudWatchBufferConfig | undefined {
    if (this.targetConfig?.buffer === undefined) return undefined;
    const buf = obj(this.targetConfig.buffer);
    const typeRaw = typeof buf.type === "string" ? buf.type.toLowerCase() : undefined;
    return {
      type: typeRaw === "memory" ? "memory" : "durable",
      path:
        typeof buf.path === "string"
          ? buf.path
          : "/var/lib/edgecommons/metrics/{ComponentName}/cw",
      maxDiskBytes: asInt(buf.maxDiskBytes) ?? 128 * 1024 * 1024,
      onFull:
        buf.onFull === "block" || buf.onFull === "rejectNew" ? buf.onFull : "dropOldest",
      fsync: buf.fsync === "interval" || buf.fsync === "always" ? buf.fsync : "perBatch",
    };
  }
}

/** Resolved `targetConfig.cloudwatch.buffer` settings for the durable CloudWatch target. */
export interface CloudWatchBufferConfig {
  type: "durable" | "memory";
  path: string;
  maxDiskBytes: number;
  onFull: "dropOldest" | "block" | "rejectNew";
  fsync: "perBatch" | "interval" | "always";
}

/**
 * `health` section — the Kubernetes-style HTTP health/readiness endpoint (Phase 1c / FR-HB-1).
 *
 * Parsed per the canonical schema. {@link enabled} is deliberately **tri-state** (`true`/`false`/
 * `undefined`): a present value overrides the platform default, while `undefined` (no `enabled` key)
 * lets the platform profile decide (on by default on KUBERNETES, off elsewhere). The remaining fields
 * carry the schema defaults (port `8081`; paths `/livez`, `/readyz`, `/startupz`).
 */
export class HealthConfig {
  /** Explicit enable/disable, or `undefined` to defer to the platform-profile default. */
  enabled?: boolean;
  /** TCP port the health server binds (0.0.0.0); default `8081`. */
  port: number;
  /** Liveness probe path (200 while the process is alive); default `/livez`. */
  livenessPath: string;
  /** Readiness probe path (200 only when connected && ready && !shuttingDown); default `/readyz`. */
  readinessPath: string;
  /** Startup probe path (reuses readiness semantics); default `/startupz`. */
  startupPath: string;

  constructor(raw: Record<string, unknown>) {
    this.enabled = typeof raw.enabled === "boolean" ? raw.enabled : undefined;
    this.port = asInt(raw.port) ?? 8081;
    this.livenessPath = typeof raw.livenessPath === "string" ? raw.livenessPath : "/livez";
    this.readinessPath = typeof raw.readinessPath === "string" ? raw.readinessPath : "/readyz";
    this.startupPath = typeof raw.startupPath === "string" ? raw.startupPath : "/startupz";
  }
}

/** `component` section. */
export interface ComponentConfig {
  global: unknown;
  instances: unknown[];
}

/** The full typed configuration view. */
export interface RawConfig {
  logging: LoggingConfig;
  heartbeat: HeartbeatConfig;
  metricEmission: MetricConfig;
  health: HealthConfig;
  tags: Record<string, unknown>;
  component: ComponentConfig;
}

/** The schema default for `messaging.requestTimeoutSeconds` (seconds). */
export const DEFAULT_REQUEST_TIMEOUT_SECONDS = 30;

/** An immutable configuration snapshot. */
export class Config {
  readonly componentName: string;
  readonly thingName: string;
  readonly parsed: RawConfig;
  /** The original JSON document (for template substitution + instance subtrees). */
  readonly raw: Record<string, unknown>;
  /**
   * The component's resolved UNS identity (hierarchy + identity values + device + component
   * token, instance {@link MessageIdentity.DEFAULT_INSTANCE}), resolved **once at
   * construction** from the component's OWN config (no shared config, UNS-CANONICAL-DESIGN
   * §1.5) — {@link fromValue} fails fast on any inconsistency.
   */
  readonly componentIdentity: MessageIdentity;
  /**
   * Whether UNS topics built by `gg.uns()` / `gg.instance(id).uns()` carry the first hierarchy
   * value (`site`) between the `ecv1` root and the device — the top-level `topic.includeRoot`
   * setting (§2.2 rule 6 / D-U11), default `false`. Effective in `Uns` only with a multi-level
   * hierarchy (D-U25).
   */
  readonly topicIncludeRoot: boolean;
  /**
   * The default `request()` deadline in seconds — `messaging.requestTimeoutSeconds` (§5 /
   * D-U5), default {@link DEFAULT_REQUEST_TIMEOUT_SECONDS}; `0` disables the default deadline.
   * Late-bound onto the messaging service by the builder right after the config loads.
   */
  readonly messagingRequestTimeoutSeconds: number;

  private constructor(
    componentName: string,
    thingName: string,
    parsed: RawConfig,
    raw: Record<string, unknown>,
    componentIdentity: MessageIdentity,
    topicIncludeRoot: boolean,
    messagingRequestTimeoutSeconds: number,
  ) {
    this.componentName = componentName;
    this.thingName = thingName;
    this.parsed = parsed;
    this.raw = raw;
    this.componentIdentity = componentIdentity;
    this.topicIncludeRoot = topicIncludeRoot;
    this.messagingRequestTimeoutSeconds = messagingRequestTimeoutSeconds;
  }

  /**
   * Build a snapshot from a raw JSON document. Fails fast (throws `Error`) when the top-level
   * `hierarchy`/`identity` sections are inconsistent (§1.5) — a startup error on first load, a
   * reject-and-keep on hot reload.
   */
  static fromValue(componentName: string, thingName: string, raw: unknown): Config {
    const r = obj(raw);
    const component = obj(r.component);
    const parsed: RawConfig = {
      logging: new LoggingConfig(obj(r.logging)),
      heartbeat: new HeartbeatConfig(obj(r.heartbeat)),
      metricEmission: new MetricConfig(obj(r.metricEmission)),
      health: new HealthConfig(obj(r.health)),
      tags: obj(r.tags),
      component: {
        global: component.global ?? null,
        instances: Array.isArray(component.instances) ? component.instances : [],
      },
    };
    const identity = resolveComponentIdentity(r, componentName, thingName);
    const includeRoot = parseTopicIncludeRoot(r);
    // D-U25: includeRoot needs a level ABOVE the device to prepend — with a single-level
    // hierarchy (the zero-config ["device"] default) hier[0] IS the device, so the setting
    // is a no-op in Uns (prepending would duplicate the device). Tell the user.
    if (includeRoot && identity.hier.length === 1) {
      logger.warn(
        "topic.includeRoot=true has no effect with a single-level hierarchy" +
          " (hierarchy.levels resolves to one level - the device): the site position requires" +
          " a level above the device, so UNS topics stay rootless." +
          " Declare a multi-level hierarchy.levels or remove topic.includeRoot.",
      );
    }
    return new Config(
      componentName,
      thingName,
      parsed,
      r,
      identity,
      includeRoot,
      parseMessagingRequestTimeoutSeconds(r),
    );
  }

  /**
   * The default `request()` deadline in milliseconds resolved from
   * `messaging.requestTimeoutSeconds` (§5 / D-U5), default 30 000. Returns `0` when the
   * configured value is `0` (default deadline disabled).
   */
  messagingRequestTimeoutMs(): number {
    return this.messagingRequestTimeoutSeconds <= 0
      ? 0
      : Math.round(this.messagingRequestTimeoutSeconds * 1000);
  }

  /** Global component config subtree (`component.global`). */
  global(): unknown {
    return this.parsed.component.global;
  }

  /** Instance ids declared under `component.instances[].id`. */
  instanceIds(): string[] {
    return this.parsed.component.instances
      .map((inst) => obj(inst).id)
      .filter((id): id is string => typeof id === "string");
  }

  /** The instance subtree whose `id` matches, if any. */
  instance(id: string): unknown | undefined {
    return this.parsed.component.instances.find((inst) => obj(inst).id === id);
  }
}

/** Strict UNS hierarchy level-name rule (future Parquet columns — keep it tight). */
const HIERARCHY_LEVEL_NAME = /^[A-Za-z0-9_-]+$/;

/** Builds the uniform fail-fast identity-resolution startup error. */
function identityError(detail: string): Error {
  return new Error(`Component identity resolution failed: ${detail}`);
}

/** Sanitizes an identity value via the template sanitizer, WARN-logging when it changed. */
function sanitizedIdentityValue(what: string, rawValue: string): string {
  const sanitized = sanitize(rawValue);
  if (sanitized !== rawValue) {
    logger.warn(
      `Identity value for '${what}' contained reserved characters and was sanitized: '${rawValue}' -> '${sanitized}'`,
    );
  }
  return sanitized;
}

/**
 * Resolves the component's UNS identity from its OWN config (UNS-CANONICAL-DESIGN §1.5 —
 * identical 4 ways, fail-fast):
 * 1. `levels` = top-level `hierarchy.levels` when present, else the zero-config default
 *    `["device"]`.
 * 2. Level names must match `^[A-Za-z0-9_-]+$`, be unique and non-empty.
 * 3. Every level except the last takes its value from the top-level `identity` config object
 *    (a missing value is a startup error naming the level); the LAST level's value is the
 *    resolved thing name (the existing identity chain — D-U1).
 * 4. An `identity` key equal to the last level name, or not among the declared non-device
 *    levels, is a startup error (typo protection the schema cannot express).
 * 5. Every value and the component token pass through the template sanitizer. The component
 *    token comes from `component.token` when configured, else the short component name fallback.
 */
function resolveComponentIdentity(
  raw: Record<string, unknown>,
  componentName: string,
  thingName: string,
): MessageIdentity {
  // 1. levels = hierarchy.levels if present, else the zero-config default ["device"].
  const levels: string[] = [];
  if ("hierarchy" in raw) {
    const hierarchy = raw.hierarchy;
    if (hierarchy === null || typeof hierarchy !== "object" || Array.isArray(hierarchy)
        || !("levels" in (hierarchy as Record<string, unknown>))) {
      throw identityError("'hierarchy' must be an object with a 'levels' array");
    }
    const levelsRaw = (hierarchy as Record<string, unknown>).levels;
    if (!Array.isArray(levelsRaw) || levelsRaw.length === 0) {
      throw identityError("'hierarchy.levels' must be a non-empty array of level names");
    }
    for (const levelRaw of levelsRaw) {
      if (typeof levelRaw !== "string") {
        throw identityError("'hierarchy.levels' entries must be strings");
      }
      levels.push(levelRaw);
    }
  } else {
    levels.push("device");
  }

  // 2. Level names: strict charset, unique, non-empty.
  const seen = new Set<string>();
  for (const level of levels) {
    if (!HIERARCHY_LEVEL_NAME.test(level)) {
      throw identityError(`invalid hierarchy level name '${level}' (must match ^[A-Za-z0-9_-]+$)`);
    }
    if (seen.has(level)) {
      throw identityError(`duplicate hierarchy level name '${level}'`);
    }
    seen.add(level);
  }
  const deviceLevel = levels[levels.length - 1];
  const valueLevels = levels.slice(0, levels.length - 1);

  // 3/4. The `identity` config object supplies every level's value except the last;
  //      keys must be exactly (a subset of) the non-device levels.
  let identityConfig: Record<string, unknown> = {};
  if ("identity" in raw) {
    const identityRaw = raw.identity;
    if (identityRaw === null || typeof identityRaw !== "object" || Array.isArray(identityRaw)) {
      throw identityError("'identity' must be an object of level-name -> value");
    }
    identityConfig = identityRaw as Record<string, unknown>;
  }
  for (const key of Object.keys(identityConfig)) {
    if (key === deviceLevel) {
      throw identityError(
        `'identity.${key}' must not be set: '${deviceLevel}' is the last hierarchy level` +
          " (the device) and its value is always the resolved thing name",
      );
    }
    if (!valueLevels.includes(key)) {
      throw identityError(
        `'identity.${key}' is not a declared hierarchy level; expected keys: [${valueLevels.join(", ")}]`,
      );
    }
  }

  const hier: HierLevel[] = [];
  const missing: string[] = [];
  for (const level of valueLevels) {
    const value = identityConfig[level];
    if (typeof value !== "string" || value === "") {
      missing.push(level);
      continue;
    }
    hier.push({ level, value: sanitizedIdentityValue(level, value) });
  }
  if (missing.length > 0) {
    throw identityError(
      `the top-level 'identity' config object is missing value(s) for hierarchy level(s)` +
        ` [${missing.join(", ")}] (hierarchy.levels = [${levels.join(", ")}]; the last level` +
        ` '${deviceLevel}' is the resolved thing name and must not be configured)`,
    );
  }

  // The device (last level) value is the resolved thing name (the resolver identity chain).
  if (typeof thingName !== "string" || thingName === "") {
    throw identityError(`the device level '${deviceLevel}' value (the resolved thing name) is not available`);
  }
  hier.push({ level: deviceLevel, value: sanitizedIdentityValue(deviceLevel, thingName) });

  // 5. component = explicit token when configured, else sanitized short name.
  if (typeof componentName !== "string" || componentName === "") {
    throw identityError("the component name is not available");
  }
  let configuredComponentToken: string | undefined;
  if ("component" in raw) {
    const componentRaw = raw.component;
    if (componentRaw === null || typeof componentRaw !== "object" || Array.isArray(componentRaw)) {
      throw identityError("'component' must be an object when configuring 'component.token'");
    }
    const token = (componentRaw as Record<string, unknown>).token;
    if (token !== undefined) {
      if (typeof token !== "string" || token === "") {
        throw identityError("'component.token' must be a non-empty string");
      }
      configuredComponentToken = token;
    }
  }
  const shortName = componentName.includes(".")
    ? componentName.slice(componentName.lastIndexOf(".") + 1)
    : componentName;
  const componentToken = sanitizedIdentityValue("component", configuredComponentToken ?? shortName);
  return new MessageIdentity(hier, componentToken, MessageIdentity.DEFAULT_INSTANCE);
}

/**
 * Parses the top-level `topic.includeRoot` flag (default `false`). Minimal config-model
 * support for the `topic` section — lenient like the other permissive subsystem sections: a
 * missing/non-object `topic` or a missing/non-boolean `includeRoot` yields the default.
 */
function parseTopicIncludeRoot(raw: Record<string, unknown>): boolean {
  return obj(raw.topic).includeRoot === true;
}

/**
 * Parses `messaging.requestTimeoutSeconds` (§5 / D-U5): a non-negative number of seconds
 * (fractions allowed by the schema), default {@link DEFAULT_REQUEST_TIMEOUT_SECONDS}. Lenient
 * like the other permissive sections — a missing/non-object `messaging` section, a
 * missing/non-number value, or a negative value (which the schema rejects at startup anyway)
 * all yield the default. `0` is a valid explicit value meaning "disabled".
 */
function parseMessagingRequestTimeoutSeconds(raw: Record<string, unknown>): number {
  const value = obj(raw.messaging).requestTimeoutSeconds;
  if (typeof value !== "number" || !Number.isFinite(value) || value < 0) {
    return DEFAULT_REQUEST_TIMEOUT_SECONDS;
  }
  return value;
}
