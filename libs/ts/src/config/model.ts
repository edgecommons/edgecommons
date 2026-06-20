/**
 * Configuration — typed model + the runtime {@link Config} snapshot.
 *
 * Mirrors the cross-language JSON schema (`logging`, `heartbeat`, `metricEmission`,
 * `tags`, `component`). {@link Config} pairs the typed view with the original JSON
 * document and the resolved component/thing identity. It is immutable; on hot
 * reload a new snapshot replaces the old one atomically.
 *
 * Numeric fields accept a JSON float (Greengrass delivers config numbers as
 * doubles, e.g. `5.0`), matching the Rust `de_lenient_opt_u64` behavior.
 */

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
  format?: string;
  fileLogging?: FileLoggingConfig;
  loggers: Record<string, string>;
  globalControl: boolean;

  constructor(raw: Record<string, unknown>) {
    this.level = typeof raw.level === "string" ? raw.level : undefined;
    this.format = typeof raw.format === "string" ? raw.format : undefined;
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
  return {
    cpu: raw.cpu === true,
    memory: raw.memory === true,
    disk: raw.disk === true,
    threads: raw.threads === true,
    files: raw.files === true,
    fds: raw.fds === true,
  };
}

/** One entry of `heartbeat.targets`. */
export interface HeartbeatTarget {
  type: string;
  config?: Record<string, unknown>;
}

/** `heartbeat` section. */
export class HeartbeatConfig {
  intervalSecs?: number;
  measures: Measures;
  targets: HeartbeatTarget[];

  constructor(raw: Record<string, unknown>) {
    this.intervalSecs = asInt(raw.intervalSecs);
    this.measures = parseMeasures(obj(raw.measures));
    this.targets = Array.isArray(raw.targets)
      ? raw.targets.map((t) => {
          const to = obj(t);
          return {
            type: typeof to.type === "string" ? to.type : "",
            config: to.config !== undefined ? obj(to.config) : undefined,
          };
        })
      : [];
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

  /** Selected target (`log`|`messaging`|`cloudwatch`|`cloudwatchcomponent`); default `log`. */
  target(): string {
    return this.target_ ?? "log";
  }

  /** CloudWatch namespace; default `ggcommons`. */
  namespace(): string {
    return this.namespace_ ?? "ggcommons";
  }

  /** `targetConfig.logFileName` template (log target); default Greengrass path. */
  logFileName(): string {
    return this.targetConfigStr("logFileName") ?? "/greengrass/v2/logs/{ComponentFullName}.metric.log";
  }

  /** `targetConfig.maxFileSize` (log target); default `10MB`. */
  maxFileSize(): string {
    return this.targetConfigStr("maxFileSize") ?? "10MB";
  }

  /** `targetConfig.topic` template; per-target default if unset. */
  topic(): string {
    const t = this.targetConfigStr("topic");
    if (t) return t;
    return this.target() === "cloudwatchcomponent"
      ? "cloudwatch/metric/put"
      : "{ThingName}/{ComponentName}/metric";
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
  tags: Record<string, unknown>;
  component: ComponentConfig;
}

/** An immutable configuration snapshot. */
export class Config {
  readonly componentName: string;
  readonly thingName: string;
  readonly parsed: RawConfig;
  /** The original JSON document (for template substitution + instance subtrees). */
  readonly raw: Record<string, unknown>;

  private constructor(componentName: string, thingName: string, parsed: RawConfig, raw: Record<string, unknown>) {
    this.componentName = componentName;
    this.thingName = thingName;
    this.parsed = parsed;
    this.raw = raw;
  }

  /** Build a snapshot from a raw JSON document. */
  static fromValue(componentName: string, thingName: string, raw: unknown): Config {
    const r = obj(raw);
    const component = obj(r.component);
    const parsed: RawConfig = {
      logging: new LoggingConfig(obj(r.logging)),
      heartbeat: new HeartbeatConfig(obj(r.heartbeat)),
      metricEmission: new MetricConfig(obj(r.metricEmission)),
      tags: obj(r.tags),
      component: {
        global: component.global ?? null,
        instances: Array.isArray(component.instances) ? component.instances : [],
      },
    };
    return new Config(componentName, thingName, parsed, r);
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
