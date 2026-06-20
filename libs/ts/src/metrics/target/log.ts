/**
 * Metrics target — log (TypeScript).
 *
 * Appends EMF JSON (one object per line) to a log file with size-based rotation.
 * Mirrors the Rust `metrics::target::log::LogTarget` (and the Java/Python `log`
 * target) exactly:
 *  - **Lazy, fail-soft open**: the file is not opened at construction. It is opened
 *    on the first emit; if it cannot be opened/written, a single `console.warn` is
 *    logged and metrics are dropped rather than throwing (matching Java's appender
 *    fallback / non-root `/greengrass/v2/logs` case).
 *  - **Rotation**: when a write would exceed `maxFileSize` (default `10MB`, parsed
 *    1024-based with B/KB/MB/GB units) the current file is renamed to
 *    `<stem>-<UTC-timestamp><ext>` and a fresh file is opened; up to 5 rolled files
 *    are retained (oldest pruned), matching Rust's `MAX_BACKUPS = 5`.
 *  - `largeFleetWorkaround` writes a second line with `coreName="ALL"`.
 *  - `emit` and `emitNow` behave identically (no batching for a file).
 *
 * Uses synchronous `fs` writes, mirroring the Rust target's simplicity (short
 * writes that never hold a lock across an await).
 */
import * as fs from "fs";
import * as path from "path";

import type { MetricTarget } from "../types";
import type { MeasureValues } from "../types";
import type { Metric } from "../metric";
import { buildEmfVariants } from "../emf";

/** Default max file size when `maxFileSize` is unset or unparseable. */
const DEFAULT_MAX_BYTES = 10 * 1024 * 1024;
/** Number of rolled files to retain (matches Rust's `MAX_BACKUPS`). */
const MAX_BACKUPS = 5;

/**
 * Parse a size string like `"10MB"`, `"512KB"`, `"1GB"`, or `"1048576"` into bytes.
 * Units are case-insensitive and 1024-based (matching Log4j2's `FileSize` and the
 * Rust `parse_size`). Returns `undefined` for unparseable input.
 */
export function parseSize(input: string): number | undefined {
  const s = input.trim();
  if (s.length === 0) {
    return undefined;
  }
  // Split leading ASCII digits from the trailing unit.
  let digitsEnd = 0;
  while (digitsEnd < s.length && s.charCodeAt(digitsEnd) >= 0x30 && s.charCodeAt(digitsEnd) <= 0x39) {
    digitsEnd += 1;
  }
  const number = s.slice(0, digitsEnd).trim();
  const unit = s.slice(digitsEnd).trim().toUpperCase();
  if (number.length === 0) {
    return undefined;
  }
  const value = Number.parseInt(number, 10);
  if (!Number.isFinite(value)) {
    return undefined;
  }
  let multiplier: number;
  switch (unit) {
    case "":
    case "B":
      multiplier = 1;
      break;
    case "KB":
    case "K":
      multiplier = 1024;
      break;
    case "MB":
    case "M":
      multiplier = 1024 * 1024;
      break;
    case "GB":
    case "G":
      multiplier = 1024 * 1024 * 1024;
      break;
    default:
      return undefined;
  }
  return value * multiplier;
}

/** Compact UTC timestamp `YYYYMMDDHHMMSS` for rolled file names. */
function timestampCompact(): string {
  const t = new Date();
  const pad = (n: number, width = 2): string => n.toString().padStart(width, "0");
  return (
    pad(t.getUTCFullYear(), 4) +
    pad(t.getUTCMonth() + 1) +
    pad(t.getUTCDate()) +
    pad(t.getUTCHours()) +
    pad(t.getUTCMinutes()) +
    pad(t.getUTCSeconds())
  );
}

/** Appends EMF JSON lines to a file, rotating by size. */
export class LogTarget implements MetricTarget {
  private readonly filePath: string;
  private readonly namespace: string;
  private readonly largeFleetWorkaround: boolean;
  private readonly maxBytes: number;

  /** Whether the file has been opened (parent dirs created) at least once. */
  private opened = false;
  /** Whether the file is unwritable; once true, emits are dropped silently. */
  private failed = false;
  /** Whether the open failure has already been warned about (warn at most once). */
  private openWarned = false;
  /** Current size of the active file in bytes. */
  private size = 0;

  /**
   * Construct a log target for `filePath`, rotating at `maxFileSize` (e.g. `"10MB"`).
   * The file is **not** opened here — it is opened lazily on the first emit.
   */
  constructor(filePath: string, namespace: string, largeFleetWorkaround: boolean, maxFileSize: string) {
    this.filePath = filePath;
    this.namespace = namespace;
    this.largeFleetWorkaround = largeFleetWorkaround;
    this.maxBytes = parseSize(maxFileSize) ?? DEFAULT_MAX_BYTES;
  }

  /**
   * Ensure the file is open, creating parent dirs and reading the current size on
   * first use. Returns `false` (fail-soft) if the file cannot be opened/created,
   * warning at most once. Matches Rust `ensure_open`.
   */
  private ensureOpen(): boolean {
    if (this.failed) {
      return false;
    }
    if (this.opened) {
      return true;
    }
    try {
      const parent = path.dirname(this.filePath);
      if (parent && parent !== ".") {
        fs.mkdirSync(parent, { recursive: true });
      }
      // Create the file if missing; capture its current size.
      const fd = fs.openSync(this.filePath, "a");
      try {
        this.size = fs.fstatSync(fd).size;
      } finally {
        fs.closeSync(fd);
      }
      this.opened = true;
      return true;
    } catch (e) {
      if (!this.openWarned) {
        // eslint-disable-next-line no-console
        console.warn(
          `metric log: cannot open file '${this.filePath}'; dropping metrics (fail-soft, matching Java): ${String(e)}`,
        );
        this.openWarned = true;
      }
      this.failed = true;
      return false;
    }
  }

  /**
   * Compute a unique timestamped backup path: `<stem>-<UTC ts>[-<n>]<ext>`,
   * disambiguating with a numeric suffix if the candidate already exists.
   */
  private rolledPath(): string {
    const dir = path.dirname(this.filePath);
    const ext = path.extname(this.filePath); // includes the leading "."
    const base = path.basename(this.filePath, ext);
    const stem = base.length > 0 ? base : "metric";
    const ts = timestampCompact();

    const build = (suffix?: number): string => {
      const name = suffix === undefined ? `${stem}-${ts}` : `${stem}-${ts}-${suffix}`;
      return path.join(dir, name + ext);
    };

    let candidate = build();
    let n = 1;
    while (fs.existsSync(candidate)) {
      candidate = build(n);
      n += 1;
    }
    return candidate;
  }

  /** Delete the oldest rolled files beyond {@link MAX_BACKUPS}. */
  private pruneBackups(): void {
    const dir = path.dirname(this.filePath);
    const ext = path.extname(this.filePath);
    const stem = path.basename(this.filePath, ext);
    const prefix = `${stem}-`;

    let entries: string[];
    try {
      entries = fs.readdirSync(dir);
    } catch {
      return;
    }

    const rolled: Array<{ mtime: number; full: string }> = [];
    for (const name of entries) {
      const full = path.join(dir, name);
      if (full === this.filePath) {
        continue;
      }
      const matchesExt = ext.length > 0 ? name.endsWith(ext) : true;
      if (name.startsWith(prefix) && matchesExt) {
        try {
          const st = fs.statSync(full);
          rolled.push({ mtime: st.mtimeMs, full });
        } catch {
          // skip unreadable entry
        }
      }
    }

    rolled.sort((a, b) => a.mtime - b.mtime); // oldest first
    const excess = Math.max(0, rolled.length - MAX_BACKUPS);
    for (let i = 0; i < excess; i += 1) {
      try {
        fs.unlinkSync(rolled[i].full);
      } catch {
        // ignore prune failures
      }
    }
  }

  /**
   * Close (logically) the current file, rename it to a timestamped backup, prune
   * old backups, and reset the active size. Matches Rust `rotate`.
   */
  private rotate(): void {
    const rolled = this.rolledPath();
    fs.renameSync(this.filePath, rolled);
    this.pruneBackups();
    this.size = 0;
  }

  /** Build EMF variants and append each as one JSON line (fail-soft). */
  private writeMetric(metric: Metric, values: MeasureValues): void {
    const variants = buildEmfVariants(this.namespace, metric, values, this.largeFleetWorkaround);
    if (!this.ensureOpen()) {
      return;
    }
    try {
      for (const emf of variants) {
        const line = JSON.stringify(emf);
        const needed = Buffer.byteLength(line, "utf8") + 1; // + newline
        if (this.size > 0 && this.size + needed > this.maxBytes) {
          this.rotate();
        }
        fs.appendFileSync(this.filePath, line + "\n");
        this.size += needed;
      }
    } catch (e) {
      // Fail-soft: a write error after open does not crash the component.
      if (!this.openWarned) {
        // eslint-disable-next-line no-console
        console.warn(
          `metric log: write to '${this.filePath}' failed; dropping metrics (fail-soft): ${String(e)}`,
        );
        this.openWarned = true;
      }
    }
  }

  async emit(metric: Metric, values: MeasureValues): Promise<void> {
    this.writeMetric(metric, values);
  }

  async emitNow(metric: Metric, values: MeasureValues): Promise<void> {
    this.writeMetric(metric, values);
  }

  async flush(): Promise<void> {
    // Synchronous appends are already durable to the OS; no buffered handle to flush.
  }

  async shutdown(): Promise<void> {
    // No persistent handle to close.
  }
}
