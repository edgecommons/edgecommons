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
 *   file writer.
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

// ---- Module-wide logging state: root level, per-logger overrides, shared sinks. ----
let rootLevel: Level = Level.INFO;
let loggersConfig: Record<string, string> = {};
let sharedFormat: string = DEFAULT_TS_FORMAT;
let sharedFileWriter: RotatingFileWriter | undefined;
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
  readonly loggerName: string;

  constructor(name = "ggcommons") {
    this.loggerName = name;
  }

  /** Recompute this logger's level (per-logger or root), format, and file sink from config. */
  refresh(): void {
    this.level = effectiveLevel(this.loggerName);
    this.format = sharedFormat;
    this.fileWriter = sharedFileWriter;
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

  private log(level: Level, message: string): void {
    if (level < this.level) {
      return;
    }
    const line = renderLine(this.format, level, message, this.loggerName);
    try {
      // Console: warn/error to stderr, everything else to stdout.
      if (level >= Level.WARN) {
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

  debug(message: string): void {
    this.log(Level.DEBUG, message);
  }
  info(message: string): void {
    this.log(Level.INFO, message);
  }
  warn(message: string): void {
    this.log(Level.WARN, message);
  }
  error(message: string): void {
    this.log(Level.ERROR, message);
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

/** Apply config to the module-wide state and refresh every live logger. */
function applyLoggingConfig(config: Config): void {
  rootLevel = parseLevel(config.parsed.logging.level);
  loggersConfig = config.parsed.logging.loggers ?? {};
  const fmt = config.parsed.logging.tsFormat;
  sharedFormat = fmt && fmt.length > 0 ? fmt : DEFAULT_TS_FORMAT;
  sharedFileWriter = buildFileWriter(config);
  for (const l of registry.values()) {
    l.refresh();
  }
}

/**
 * Initialize logging from `config`: root level (`logging.level`), per-logger overrides
 * (`logging.loggers`), format (`logging.ts_format`), and the rotating file writer. Fail-soft.
 */
export function initLogging(config: Config): void {
  applyLoggingConfig(config);
}

/**
 * Re-apply level (root + per-logger), format, and the rotating file writer on hot reload.
 * (Node lets us rebuild the file writer live, unlike the Rust tracing-layer limitation.)
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
