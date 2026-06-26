/**
 * Logging — a minimal leveled logger with optional size-rotating file output.
 *
 * **One-liner purpose**: Initialize a process-wide logger from config, with a
 * runtime-reloadable log level and optional rotating file output. Mirrors the
 * Rust `logging.rs` intent: the level comes from `logging.level` (default INFO),
 * file logging is decided at `init` time (a `fileLogging` change on hot reload
 * does not re-create the file layer — only the level reconfigures live), and a
 * {@link LoggingReconfigurer} re-applies the level on config hot reload.
 *
 * TypeScript has no `tracing` crate, so this implements a small dependency-free
 * (Node `fs` only) leveled logger instead of installing a global subscriber.
 *
 * ## Parity notes
 * - Level filtering matches Rust: an unparseable level falls back to INFO.
 * - File rotation mirrors the Python `RotatingFileHandler` / Rust
 *   `RotatingFileWriter`: rotate `<path>` → `<path>.1` … `<path>.N`, dropping the
 *   oldest, keeping `backupCount` backups; `maxBytes == 0` disables rotation;
 *   `backupCount == 0` discards the old file on rollover; an empty file is never
 *   rotated (so a single oversized record still lands).
 * - `parseFileSize` matches the Python/Rust `_parse_file_size`: `B`/`KB`/`MB`/`GB`
 *   suffixes (case-insensitive, 1024-based); unparseable falls back to `10MB`.
 * - `logging.ts_format` IS applied (token template: {timestamp}/{level}/{logger}/{message});
 *   it replaces the former language-agnostic `format`, re-applied on hot reload with the level and
 *   file writer. The special selector value `json` (case-insensitive, {@link JSON_LOG_FORMAT}) instead
 *   selects the structured **stdout-JSON sink** (Phase 1c / FR-LOG-1): one JSON object per line. This
 *   sink is the platform-profile default on KUBERNETES (threaded in via {@link initLogging}'s
 *   `formatDefault`); precedence is explicit `logging.ts_format` ▸ profile default ▸ library default.
 *   When the JSON sink is active, in-process file rotation is NOT installed (FR-LOG-2 — the cluster log
 *   agent owns rotation; stdout-only also survives a read-only root FS), and best-effort correlation
 *   fields (pod/namespace/node from the Downward-API env vars, thing from the resolved identity) are
 *   added to each line when present (FR-LOG-3).
 * - `logging.loggers` (per-logger levels) IS applied: {@link getLogger} returns a named logger
 *   whose level is the longest dotted-prefix match in `logging.loggers`, else the root level
 *   (mirrors Python's logging hierarchy / Rust's EnvFilter targets).
 * - Logging never throws (fail-soft): file errors are reported to stderr and file
 *   logging is skipped/aborted, never propagated.
 */
import * as fs from "fs";
import * as path from "path";

import type { Config } from "./config/model";
import { resolve } from "./config/template";
import type { ConfigurationChangeListener } from "./config";
import {
  Env,
  ENV_K8S_NODE_NAME,
  ENV_K8S_POD_NAME,
  ENV_K8S_POD_NAMESPACE,
  JSON_LOG_FORMAT,
} from "./platform";

/** Severity levels, ordered low → high. */
enum Level {
  DEBUG = 0,
  INFO = 1,
  WARN = 2,
  ERROR = 3,
}

/** Parse a level name (case-insensitive); unknown falls back to INFO (like Rust). */
function parseLevel(name: string | undefined): Level {
  switch ((name ?? "INFO").trim().toUpperCase()) {
    case "DEBUG":
    case "TRACE":
      return Level.DEBUG;
    case "INFO":
      return Level.INFO;
    case "WARN":
    case "WARNING":
      return Level.WARN;
    case "ERROR":
      return Level.ERROR;
    default:
      return Level.INFO;
  }
}

/** Default max bytes when a size string cannot be parsed (matches Python/Rust). */
const DEFAULT_MAX_BYTES = 10 * 1024 * 1024;

/**
 * Parse a size string like `10MB`, `512KB`, `1GB`, or `4096` into bytes.
 *
 * Recognizes `B`/`KB`/`MB`/`GB` suffixes (case-insensitive, 1024-based). A bare
 * number is bytes. Falls back to `10MB` when unparseable — matching the Python
 * library's `_parse_file_size` and the Rust `parse_file_size`.
 */
function parseFileSize(value: string): number {
  const up = value.trim().toUpperCase();
  // Longer suffixes first so "10MB" is not matched by the bare "B".
  const units: Array<[string, number]> = [
    ["KB", 1024],
    ["MB", 1024 * 1024],
    ["GB", 1024 * 1024 * 1024],
    ["B", 1],
  ];
  for (const [suffix, multiplier] of units) {
    if (up.endsWith(suffix)) {
      const num = up.slice(0, up.length - suffix.length).trim();
      const v = Number(num);
      if (num !== "" && Number.isInteger(v) && v >= 0) {
        return v * multiplier;
      }
    }
  }
  // A bare number (no suffix) is treated as bytes.
  const bare = Number(up);
  if (up !== "" && Number.isInteger(bare) && bare >= 0) {
    return bare;
  }
  return DEFAULT_MAX_BYTES;
}

/**
 * A size-rotating file writer: appends to `path`, and when a write would push the
 * file past `maxBytes` it rotates `path` → `path.1`, shifting older backups up to
 * `backupCount` and discarding the oldest. `maxBytes == 0` disables rotation;
 * `backupCount == 0` discards the old file on rollover. Mirrors the Rust
 * `RotatingFileWriter`. Never throws — I/O errors are reported to stderr.
 */
class RotatingFileWriter {
  private currentSize: number;

  constructor(
    private readonly filePath: string,
    private readonly maxBytes: number,
    private readonly backupCount: number,
  ) {
    let size = 0;
    try {
      size = fs.statSync(filePath).size;
    } catch {
      size = 0;
    }
    this.currentSize = size;
  }

  /** `<path>.N` backup name. */
  private backupPath(n: number): string {
    return `${this.filePath}.${n}`;
  }

  /** Close the active file conceptually, shift backups, and reset size. */
  private rotate(): void {
    try {
      if (this.backupCount === 0) {
        // No backups kept: discard the old content.
        safeRemove(this.filePath);
      } else {
        // Drop the oldest backup, then shift the rest up by one.
        safeRemove(this.backupPath(this.backupCount));
        for (let i = this.backupCount - 1; i >= 1; i--) {
          const src = this.backupPath(i);
          if (fs.existsSync(src)) {
            safeRename(src, this.backupPath(i + 1));
          }
        }
        if (fs.existsSync(this.filePath)) {
          safeRename(this.filePath, this.backupPath(1));
        }
      }
    } catch (e) {
      reportError(`failed to rotate log file ${this.filePath}`, e);
    }
    this.currentSize = 0;
  }

  /** Append a line (the writer adds no newline; the caller includes one). */
  write(text: string): void {
    const bytes = Buffer.byteLength(text, "utf8");
    // Rotate before writing if this write would exceed the limit (but never
    // rotate an empty file, so a single oversized record still lands).
    if (this.maxBytes > 0 && this.currentSize > 0 && this.currentSize + bytes > this.maxBytes) {
      this.rotate();
    }
    try {
      fs.appendFileSync(this.filePath, text);
      this.currentSize += bytes;
    } catch (e) {
      reportError(`failed to write log file ${this.filePath}`, e);
    }
  }
}

function safeRemove(p: string): void {
  try {
    fs.rmSync(p, { force: true });
  } catch {
    /* fail-soft */
  }
}

function safeRename(src: string, dst: string): void {
  try {
    fs.renameSync(src, dst);
  } catch {
    /* fail-soft */
  }
}

/** Report a logging-internal error to stderr without throwing. */
function reportError(message: string, err: unknown): void {
  try {
    const detail = err instanceof Error ? err.message : String(err);
    process.stderr.write(`ggcommons: ${message}: ${detail}\n`);
  } catch {
    /* fail-soft */
  }
}

/**
 * Build the rotating-file writer if file logging is enabled and the file can be
 * opened; otherwise `undefined`. Errors are reported to stderr (fail-soft).
 * Mirrors the Rust `file_make_writer`.
 */
function buildFileWriter(config: Config): RotatingFileWriter | undefined {
  const fileLogging = config.parsed.logging.fileLogging;
  if (!fileLogging || !fileLogging.enabled) {
    return undefined;
  }
  const rawPath = fileLogging.filePath;
  if (!rawPath) {
    return undefined;
  }
  const resolvedPath = resolve(config, rawPath);
  const maxBytes = parseFileSize(fileLogging.maxFileSize());
  const backupCount = fileLogging.backupCount();

  const parent = path.dirname(resolvedPath);
  if (parent && parent !== ".") {
    try {
      fs.mkdirSync(parent, { recursive: true });
    } catch (e) {
      reportError(`failed to create log directory ${parent}`, e);
      return undefined;
    }
  }

  try {
    // Touch the file so it exists (matching the Rust open-on-init behavior).
    fs.appendFileSync(resolvedPath, "");
    return new RotatingFileWriter(resolvedPath, maxBytes, backupCount);
  } catch (e) {
    reportError(`failed to open log file ${resolvedPath}`, e);
    return undefined;
  }
}

/** Default `ts_format` when none is configured: ISO-8601 timestamp + `[LEVEL] message`. */
const DEFAULT_TS_FORMAT = "{timestamp} [{level}] {message}";

/**
 * Render a log line from a `ts_format` token template. Supported tokens:
 * `{timestamp}` (ISO-8601 UTC), `{level}`, `{logger}` (logger name), `{message}`.
 * Unknown `{...}` tokens are left as-is.
 */
function renderLine(format: string, level: Level, message: string, loggerName: string): string {
  return format
    .replace(/\{timestamp\}/g, new Date().toISOString())
    .replace(/\{level\}/g, Level[level])
    .replace(/\{logger\}/g, loggerName)
    .replace(/\{message\}/g, message);
}

/**
 * Best-effort logging correlation fields (FR-LOG-3) added to each stdout-JSON line. Sourced from the
 * Kubernetes Downward-API env vars ({@link ENV_K8S_POD_NAME}/{@link ENV_K8S_POD_NAMESPACE}/
 * {@link ENV_K8S_NODE_NAME} — the same vars wired in Phase 1b) and the resolved identity (`thing`).
 * An absent value is `undefined` and is omitted from the JSON object (no empty/null noise).
 */
interface Correlation {
  pod?: string;
  namespace?: string;
  node?: string;
  thing?: string;
}

/** Read `env[key]` if present and non-empty, else `undefined` (treats `""` as absent). */
function envNonEmpty(env: Env, key: string): string | undefined {
  const v = env[key];
  return v !== undefined && v !== "" ? v : undefined;
}

/** Capture the correlation fields from the environment + resolved identity (best-effort). */
function captureCorrelation(env: Env, thingName: string): Correlation {
  return {
    pod: envNonEmpty(env, ENV_K8S_POD_NAME),
    namespace: envNonEmpty(env, ENV_K8S_POD_NAMESPACE),
    node: envNonEmpty(env, ENV_K8S_NODE_NAME),
    thing: thingName && thingName.length > 0 ? thingName : undefined,
  };
}

/** Render an error/exception value into a string for the JSON `thrown` field (stack when available). */
function errorToString(err: unknown): string {
  if (err instanceof Error) {
    return err.stack ?? `${err.name}: ${err.message}`;
  }
  return String(err);
}

/**
 * Render one line of the structured stdout-JSON sink (FR-LOG-1): a single JSON object with at least
 * `timestamp`, `level`, `logger`, `message`, plus any present correlation fields (FR-LOG-3) and a
 * `thrown` field when an error is supplied. `JSON.stringify` yields no embedded newlines (control chars
 * are escaped), so the result is always exactly one line of valid JSON.
 */
function renderJsonLine(
  level: Level,
  message: string,
  loggerName: string,
  corr: Correlation,
  error?: unknown,
): string {
  const obj: Record<string, unknown> = {
    timestamp: new Date().toISOString(),
    level: Level[level],
    logger: loggerName,
    message,
  };
  if (corr.pod !== undefined) obj.pod = corr.pod;
  if (corr.namespace !== undefined) obj.namespace = corr.namespace;
  if (corr.node !== undefined) obj.node = corr.node;
  if (corr.thing !== undefined) obj.thing = corr.thing;
  if (error !== undefined) obj.thrown = errorToString(error);
  return JSON.stringify(obj);
}

// ---- Module-wide logging state: root level, per-logger overrides, shared sinks. ----
let rootLevel: Level = Level.INFO;
let loggersConfig: Record<string, string> = {};
let sharedFormat: string = DEFAULT_TS_FORMAT;
let sharedFileWriter: RotatingFileWriter | undefined;
/** Whether the structured stdout-JSON sink is active (FR-LOG-1); set by {@link applyLoggingConfig}. */
let sharedJsonMode = false;
/** Best-effort correlation fields for the JSON sink (FR-LOG-3); refreshed on each config apply. */
let sharedCorrelation: Correlation = {};
/**
 * The platform-profile default logging format (e.g. `json` on KUBERNETES), threaded in via
 * {@link initLogging} and preserved across hot reloads so {@link reconfigureLogging} can re-apply it.
 */
let moduleFormatDefault: string | undefined;
/** Environment used to source correlation fields (default `process.env`; injectable for tests). */
let moduleEnv: Env = process.env;
const registry = new Map<string, Logger>();

/**
 * Resolve a logger's effective level from `logging.loggers`: longest dotted-prefix match
 * (so `a.b.c` is covered by an `a.b` entry), falling back to the root level. Mirrors Python's
 * logging hierarchy and Rust's EnvFilter target directives.
 */
function effectiveLevel(name: string): Level {
  if (name) {
    const parts = name.split(".");
    for (let i = parts.length; i > 0; i--) {
      const key = parts.slice(0, i).join(".");
      if (Object.prototype.hasOwnProperty.call(loggersConfig, key)) {
        return parseLevel(loggersConfig[key]);
      }
    }
  }
  return rootLevel;
}

/**
 * A leveled logger. Level (root + per-logger overrides), format, and the rotating file writer
 * are all refreshed from config on init and hot reload.
 */
export class Logger {
  private level: Level = Level.INFO;
  private fileWriter?: RotatingFileWriter;
  private format: string = DEFAULT_TS_FORMAT;
  private jsonMode = false;
  private correlation: Correlation = {};
  readonly loggerName: string;

  constructor(name = "ggcommons") {
    this.loggerName = name;
  }

  /** Recompute this logger's level (per-logger or root), format, JSON mode, and file sink from config. */
  refresh(): void {
    this.level = effectiveLevel(this.loggerName);
    this.format = sharedFormat;
    this.fileWriter = sharedFileWriter;
    this.jsonMode = sharedJsonMode;
    this.correlation = sharedCorrelation;
  }

  /** Set the active level threshold (overridden by the next config refresh). */
  setLevel(level: Level): void {
    this.level = level;
  }

  /** Set the `ts_format` token template (empty/undefined → the default format). */
  setFormat(format: string | undefined): void {
    this.format = format && format.length > 0 ? format : DEFAULT_TS_FORMAT;
  }

  /** Install (or clear) the size-rotating file writer. */
  setFileWriter(writer: RotatingFileWriter | undefined): void {
    this.fileWriter = writer;
  }

  private log(level: Level, message: string, error?: unknown): void {
    if (level < this.level) {
      return;
    }
    let line: string;
    try {
      if (this.jsonMode) {
        line = renderJsonLine(level, message, this.loggerName, this.correlation, error);
      } else {
        // Text mode (unchanged token-template behavior). An error, when supplied, is appended inline
        // so it is not lost (no existing call site passes one, so existing output is byte-identical).
        const text = error !== undefined ? `${message}: ${errorToString(error)}` : message;
        line = renderLine(this.format, level, text, this.loggerName);
      }
    } catch {
      return; /* fail-soft: never let a render error escape */
    }
    try {
      // JSON sink (FR-LOG-1): a single structured stream on stdout for all levels (mirrors the Java
      // canonical single SYSTEM_OUT console appender). Text mode keeps today's split: warn/error to
      // stderr, everything else to stdout.
      if (!this.jsonMode && level >= Level.WARN) {
        process.stderr.write(line + "\n");
      } else {
        process.stdout.write(line + "\n");
      }
    } catch {
      /* fail-soft */
    }
    if (this.fileWriter) {
      this.fileWriter.write(line + "\n");
    }
  }

  debug(message: string, error?: unknown): void {
    this.log(Level.DEBUG, message, error);
  }
  info(message: string, error?: unknown): void {
    this.log(Level.INFO, message, error);
  }
  warn(message: string, error?: unknown): void {
    this.log(Level.WARN, message, error);
  }
  error(message: string, error?: unknown): void {
    this.log(Level.ERROR, message, error);
  }
}

/** The process-wide (root) logger used across the library (default INFO). */
export const logger = new Logger("ggcommons");
registry.set(logger.loggerName, logger);

/**
 * Get (or create) a named logger. Its level is resolved from `logging.loggers` (per-logger
 * overrides, hierarchical) falling back to the root `logging.level`; format and file sink are
 * shared. The instance is stable across calls and stays in sync with config hot reloads.
 */
export function getLogger(name: string): Logger {
  let existing = registry.get(name);
  if (!existing) {
    existing = new Logger(name);
    registry.set(name, existing);
    existing.refresh();
  }
  return existing;
}

/**
 * The effective logging format selector (FR-RT-3 precedence): explicit `logging.ts_format` ▸ the
 * platform-profile default ({@link moduleFormatDefault}, `json` on KUBERNETES) ▸ the library default
 * ({@link DEFAULT_TS_FORMAT}). The result is either the special {@link JSON_LOG_FORMAT} token (selects
 * the stdout-JSON sink) or a text token template.
 */
function effectiveFormat(config: Config): string {
  const fmt = config.parsed.logging.tsFormat;
  if (fmt && fmt.length > 0) return fmt; // explicit config wins
  if (moduleFormatDefault && moduleFormatDefault.length > 0) return moduleFormatDefault; // profile default
  return DEFAULT_TS_FORMAT; // library default
}

/** Apply config to the module-wide state and refresh every live logger. */
function applyLoggingConfig(config: Config): void {
  rootLevel = parseLevel(config.parsed.logging.level);
  loggersConfig = config.parsed.logging.loggers ?? {};

  const fmt = effectiveFormat(config);
  sharedJsonMode = fmt.trim().toLowerCase() === JSON_LOG_FORMAT;
  // In text mode the format is the token template; in JSON mode the template is unused.
  sharedFormat = sharedJsonMode ? DEFAULT_TS_FORMAT : fmt;
  // FR-LOG-2: under the JSON sink, do NOT install in-process file rotation (the cluster log agent owns
  // rotation; stdout-only also keeps logging alive on a read-only root FS). File logging stays
  // available off the JSON sink.
  sharedFileWriter = sharedJsonMode ? undefined : buildFileWriter(config);
  sharedCorrelation = sharedJsonMode ? captureCorrelation(moduleEnv, config.thingName) : {};

  for (const l of registry.values()) {
    l.refresh();
  }
}

/** Options threaded into the logging configurator from the resolved runtime profile (Phase 1c). */
export interface LoggingOptions {
  /**
   * The platform-profile default logging format (e.g. {@link JSON_LOG_FORMAT} on KUBERNETES), applied
   * when the component config sets no `logging.ts_format`. Stored and re-applied across hot reloads.
   */
  formatDefault?: string;
  /** Environment for the JSON-sink correlation fields (default `process.env`; injectable for tests). */
  env?: Env;
}

/**
 * Initialize logging from `config`: root level (`logging.level`), per-logger overrides
 * (`logging.loggers`), the effective format (`logging.ts_format` ▸ profile default ▸ library default),
 * the stdout-JSON sink when `json` is selected (FR-LOG-1), and the rotating file writer otherwise.
 * Fail-soft. `options.formatDefault`/`options.env` (the resolved platform's logging default + the
 * correlation environment) are stored and reused on subsequent {@link reconfigureLogging} calls.
 */
export function initLogging(config: Config, options?: LoggingOptions): void {
  moduleFormatDefault = options?.formatDefault;
  moduleEnv = options?.env ?? process.env;
  applyLoggingConfig(config);
}

/**
 * Re-apply level (root + per-logger), the effective format/sink, and the rotating file writer on hot
 * reload, preserving the platform-profile default + correlation environment captured at
 * {@link initLogging}. (Node lets us rebuild the file writer live, unlike the Rust tracing-layer
 * limitation; the JSON sink likewise re-selects cleanly on reload.)
 */
export function reconfigureLogging(config: Config): void {
  applyLoggingConfig(config);
}

/**
 * A {@link ConfigurationChangeListener} that re-applies the log level on config
 * hot reload. Mirrors the Rust `LoggingReconfigurer`.
 */
export class LoggingReconfigurer implements ConfigurationChangeListener {
  onConfigurationChange(config: Config): boolean {
    reconfigureLogging(config);
    return true;
  }
}
