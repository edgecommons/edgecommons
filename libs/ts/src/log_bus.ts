/**
 * Log bus publishing — library-owned UNS `log` class.
 *
 * Publishes structured log records to
 * `ecv1[/{site}]/{device}/{component}/main/log/{level}` through the privileged
 * reserved-publish seam. The public messaging guard still rejects raw component
 * publishes to the reserved `log` class.
 */
import type { Config } from "./config/model";
import type { ConfigurationChangeListener } from "./config";
import type { LoggerSinkRecord } from "./logging";
import { addLoggerSink } from "./logging";
import { MessageBuilder } from "./message";
import type { IMessagingService } from "./messaging/types";
import { publishReservedVia } from "./messaging/service";
import { Uns, UnsClass } from "./uns";

export type LogLevelName =
  | "TRACE"
  | "DEBUG"
  | "INFO"
  | "WARN"
  | "ERROR"
  | "FATAL"
  | "trace"
  | "debug"
  | "info"
  | "warn"
  | "warning"
  | "error"
  | "fatal";

export interface LogRecord {
  timestamp?: string | number | Date;
  level: LogLevelName;
  logger: string;
  message: string;
  sequence?: number;
  thread?: string;
  fields?: Record<string, unknown>;
  error?: unknown;
  truncated?: boolean;
  dropped?: number;
}

export interface LogPublishStats {
  queued: number;
  published: number;
  dropped: number;
  failed: number;
  truncated: number;
  redacted: number;
}

export interface LogService {
  publish(record: LogRecord): Promise<void>;
  flush(): Promise<void>;
  stats(): LogPublishStats;
}

const LOG_MESSAGE_NAME = "log";
const LOG_MESSAGE_VERSION = "1.0";
const LOG_BODY_SCHEMA = "edgecommons.log.v1";

type UppercaseLogLevel = "TRACE" | "DEBUG" | "INFO" | "WARN" | "ERROR" | "FATAL";

const LEVEL_ORDER: Record<UppercaseLogLevel, number> = {
  TRACE: 0,
  DEBUG: 1,
  INFO: 2,
  WARN: 3,
  ERROR: 4,
  FATAL: 5,
};

interface PreparedRecord {
  record: LogRecord;
  resolve: () => void;
  reject: (err: unknown) => void;
}

interface CompiledRedaction {
  enabled: boolean;
  replacement: string;
  patterns: RegExp[];
}

export class LogBusService implements LogService, ConfigurationChangeListener {
  private queue: PreparedRecord[] = [];
  private draining = false;
  private sequence = 0;
  private droppedSinceLastPublish = 0;
  private readonly statsValue: LogPublishStats = {
    queued: 0,
    published: 0,
    dropped: 0,
    failed: 0,
    truncated: 0,
    redacted: 0,
  };
  private waiters: Array<() => void> = [];
  private removeLoggerSink?: () => void;
  private removeConsolePatch?: () => void;
  private redaction: CompiledRedaction = compileRedaction(undefined);
  private publishingDepth = 0;

  constructor(
    private readonly configProvider: () => Config,
    private readonly messaging: IMessagingService | undefined,
  ) {
    this.applyConfig(configProvider());
  }

  publish(record: LogRecord): Promise<void> {
    const cfg = this.configProvider().parsed.logging.publish;
    if (!cfg.enabled || !this.messaging || !levelEnabled(record.level, cfg.minLevel)) {
      return Promise.resolve();
    }
    return new Promise<void>((resolve, reject) => {
      this.enqueue({ record: { ...record }, resolve, reject });
    });
  }

  async flush(): Promise<void> {
    if (!this.draining && this.queue.length === 0) {
      return;
    }
    await new Promise<void>((resolve) => this.waiters.push(resolve));
  }

  stats(): LogPublishStats {
    return { ...this.statsValue, queued: this.queue.length };
  }

  onConfigurationChange(config: Config): boolean {
    this.applyConfig(config);
    return true;
  }

  close(): void {
    this.removeLoggerSink?.();
    this.removeLoggerSink = undefined;
    this.removeConsolePatch?.();
    this.removeConsolePatch = undefined;
  }

  private applyConfig(config: Config): void {
    const cfg = config.parsed.logging.publish;
    this.redaction = compileRedaction(cfg.redaction);

    if (cfg.enabled && cfg.captureNative) {
      if (this.removeLoggerSink === undefined) {
        this.removeLoggerSink = addLoggerSink((record) => {
          if (this.publishingDepth > 0) return;
          void this.publish({
            timestamp: record.timestamp,
            level: record.level,
            logger: record.logger,
            message: record.message,
            error: record.error,
          }).catch(() => undefined);
        });
      }
    } else {
      this.removeLoggerSink?.();
      this.removeLoggerSink = undefined;
    }

    if (cfg.enabled && cfg.captureConsole) {
      if (this.removeConsolePatch === undefined) {
        this.removeConsolePatch = patchConsole((record) => {
          if (this.publishingDepth > 0) return;
          void this.publish(record).catch(() => undefined);
        });
      }
    } else {
      this.removeConsolePatch?.();
      this.removeConsolePatch = undefined;
    }
  }

  private enqueue(item: PreparedRecord): void {
    const cfg = this.configProvider().parsed.logging.publish;
    const maxRecords = Math.max(1, cfg.queue.maxRecords);
    if (this.queue.length >= maxRecords) {
      const dropped = this.queue.shift();
      dropped?.resolve();
      this.statsValue.dropped++;
      this.droppedSinceLastPublish++;
    }
    this.queue.push(item);
    this.statsValue.queued = this.queue.length;
    this.pump();
  }

  private pump(): void {
    if (this.draining) return;
    this.draining = true;
    queueMicrotask(() => {
      void this.drain();
    });
  }

  private async drain(): Promise<void> {
    try {
      while (this.queue.length > 0) {
        const item = this.queue.shift()!;
        this.statsValue.queued = this.queue.length;
        try {
          await this.publishOne(item.record);
          item.resolve();
          this.statsValue.published++;
        } catch (e) {
          this.statsValue.failed++;
          item.reject(e);
        }
      }
    } finally {
      this.draining = false;
      if (this.queue.length > 0) {
        this.pump();
      } else {
        this.resolveWaiters();
      }
    }
  }

  private async publishOne(record: LogRecord): Promise<void> {
    const messaging = this.messaging;
    if (!messaging) return;
    if (!messaging.connected()) {
      throw new Error("log publishing skipped because messaging is disconnected");
    }
    const config = this.configProvider();
    const publishConfig = config.parsed.logging.publish;
    const timestamp = normalizeTimestamp(record.timestamp);
    const level = normalizeLevel(record.level);
    const dropped = this.droppedSinceLastPublish + (record.dropped ?? 0);
    this.droppedSinceLastPublish = 0;
    const sequence = record.sequence ?? ++this.sequence;

    const body = this.prepareBody({
      schema: LOG_BODY_SCHEMA,
      timestamp,
      level,
      logger: record.logger,
      message: record.message,
      sequence,
      ...(record.thread !== undefined ? { thread: record.thread } : {}),
      ...(record.fields !== undefined ? { fields: record.fields } : {}),
      ...(record.error !== undefined ? { error: errorValue(record.error) } : {}),
      ...(record.truncated === true ? { truncated: true } : {}),
      ...(dropped > 0 ? { dropped } : {}),
    });

    const topic = new Uns(config.componentIdentity, config.topicIncludeRoot).topic(
      UnsClass.Log,
      level.toLowerCase(),
    );
    const msg = MessageBuilder.create(LOG_MESSAGE_NAME, LOG_MESSAGE_VERSION)
      .withTimestamp(timestamp)
      .withPayload(body)
      .withConfig(config)
      .build();
    this.publishingDepth++;
    try {
      await publishReservedVia(messaging, topic, msg, publishConfig.destination);
    } finally {
      this.publishingDepth--;
    }
  }

  private prepareBody(body: Record<string, unknown>): Record<string, unknown> {
    const publishConfig = this.configProvider().parsed.logging.publish;
    const redacted = this.redactValue(body) as Record<string, unknown>;
    if (fits(redacted, publishConfig.maxRecordBytes)) {
      return redacted;
    }

    this.statsValue.truncated++;
    redacted.truncated = true;
    let message = String(redacted.message ?? "");
    while (!fits(redacted, publishConfig.maxRecordBytes) && message.length > 0) {
      const over = Buffer.byteLength(JSON.stringify(redacted), "utf8") - publishConfig.maxRecordBytes;
      const remove = Math.max(1, Math.min(message.length, over + 3));
      message = message.slice(0, Math.max(0, message.length - remove));
      redacted.message = `${message}...`;
    }
    if (!fits(redacted, publishConfig.maxRecordBytes)) {
      delete redacted.fields;
    }
    if (!fits(redacted, publishConfig.maxRecordBytes)) {
      delete redacted.error;
    }
    return redacted;
  }

  private redactValue(value: unknown): unknown {
    if (!this.redaction.enabled) return value;
    if (typeof value === "string") {
      let out = value;
      for (const pattern of this.redaction.patterns) {
        out = out.replace(pattern, () => {
          this.statsValue.redacted++;
          return this.redaction.replacement;
        });
      }
      return out;
    }
    if (Array.isArray(value)) {
      return value.map((v) => this.redactValue(v));
    }
    if (value !== null && typeof value === "object") {
      const out: Record<string, unknown> = {};
      for (const [key, inner] of Object.entries(value as Record<string, unknown>)) {
        if (isSensitiveKey(key)) {
          if (inner !== this.redaction.replacement) this.statsValue.redacted++;
          out[key] = this.redaction.replacement;
        } else {
          out[key] = this.redactValue(inner);
        }
      }
      return out;
    }
    return value;
  }

  private resolveWaiters(): void {
    const waiters = this.waiters;
    this.waiters = [];
    for (const waiter of waiters) waiter();
  }
}

function normalizeLevel(level: LogLevelName): UppercaseLogLevel {
  const value = String(level).toUpperCase();
  if (value === "WARNING") return "WARN";
  if (value in LEVEL_ORDER) return value as UppercaseLogLevel;
  return "INFO";
}

function levelEnabled(level: LogLevelName, minLevel: UppercaseLogLevel): boolean {
  return LEVEL_ORDER[normalizeLevel(level)] >= LEVEL_ORDER[minLevel];
}

function normalizeTimestamp(value: string | number | Date | undefined): string {
  if (value instanceof Date) return value.toISOString();
  if (typeof value === "number" && Number.isFinite(value)) return new Date(value).toISOString();
  if (typeof value === "string" && value.length > 0) return value;
  return new Date().toISOString();
}

function errorValue(error: unknown): unknown {
  if (error instanceof Error) {
    return {
      name: error.name,
      message: error.message,
      stack: error.stack,
    };
  }
  return error;
}

function fits(body: Record<string, unknown>, maxBytes: number): boolean {
  return Buffer.byteLength(JSON.stringify(body), "utf8") <= maxBytes;
}

function isSensitiveKey(key: string): boolean {
  return /^(password|passwd|pwd|secret|token|api[_-]?key|pin)$/i.test(key);
}

function compileRedaction(
  cfg: Config["parsed"]["logging"]["publish"]["redaction"] | undefined,
): CompiledRedaction {
  const replacement = cfg?.replacement ?? "***";
  const patterns: RegExp[] = [];
  for (const raw of cfg?.extraPatterns ?? []) {
    try {
      patterns.push(new RegExp(raw, "g"));
    } catch {
      /* Invalid operator-supplied patterns are ignored fail-soft. */
    }
  }
  return {
    enabled: cfg?.enabled ?? true,
    replacement,
    patterns,
  };
}

type ConsoleMethod = "trace" | "debug" | "info" | "log" | "warn" | "error";

const CONSOLE_LEVELS: Record<ConsoleMethod, UppercaseLogLevel> = {
  trace: "TRACE",
  debug: "DEBUG",
  info: "INFO",
  log: "INFO",
  warn: "WARN",
  error: "ERROR",
};

function patchConsole(offer: (record: LogRecord) => void): () => void {
  const methods = Object.keys(CONSOLE_LEVELS) as ConsoleMethod[];
  const originals = new Map<ConsoleMethod, (...args: unknown[]) => void>();
  for (const method of methods) {
    originals.set(method, console[method].bind(console));
    console[method] = (...args: unknown[]): void => {
      originals.get(method)?.(...args);
      offer({
        level: CONSOLE_LEVELS[method],
        logger: "console",
        message: args.map(formatConsoleArg).join(" "),
        timestamp: new Date().toISOString(),
      });
    };
  }
  return () => {
    for (const method of methods) {
      const original = originals.get(method);
      if (original) console[method] = original;
    }
  };
}

function formatConsoleArg(value: unknown): string {
  if (typeof value === "string") return value;
  if (value instanceof Error) return value.stack ?? `${value.name}: ${value.message}`;
  try {
    return JSON.stringify(value);
  } catch {
    return String(value);
  }
}
