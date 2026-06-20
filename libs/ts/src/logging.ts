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
 * - Custom format strings and per-logger levels are parsed from config but not
 *   applied (same limitation as Rust).
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

/** ISO-8601 timestamp + `[LEVEL] message`, the line written to console/file. */
function formatLine(level: Level, message: string): string {
  return `${new Date().toISOString()} [${Level[level]}] ${message}`;
}

/**
 * A leveled logger. The level threshold is mutable (live reconfiguration); the
 * file writer is fixed at init time (matching Rust's tracing-layer limitation).
 */
export class Logger {
  private level: Level = Level.INFO;
  private fileWriter?: RotatingFileWriter;

  /** Set the active level threshold. */
  setLevel(level: Level): void {
    this.level = level;
  }

  /** Install (or clear) the size-rotating file writer. */
  setFileWriter(writer: RotatingFileWriter | undefined): void {
    this.fileWriter = writer;
  }

  private log(level: Level, message: string): void {
    if (level < this.level) {
      return;
    }
    const line = formatLine(level, message);
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

/** The process-wide logger used across the library (default INFO). */
export const logger = new Logger();

/**
 * Initialize the global logger from `config`: set the level from
 * `logging.level` (default INFO, case-insensitive) and, if
 * `logging.fileLogging.enabled`, install a size-rotating file writer. Fail-soft.
 */
export function initLogging(config: Config): void {
  logger.setLevel(parseLevel(config.parsed.logging.level));
  logger.setFileWriter(buildFileWriter(config));
}

/**
 * Re-apply the log level from `config` to the running logger. The file layer is
 * fixed at {@link initLogging} time (like Rust) and is not re-created here.
 */
export function reconfigureLogging(config: Config): void {
  logger.setLevel(parseLevel(config.parsed.logging.level));
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
