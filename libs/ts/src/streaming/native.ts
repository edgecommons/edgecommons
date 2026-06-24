/**
 * Bridges the `ggstreamlog-node` napi addon (the shared Rust core, built as a native Node addon)
 * into the library: error translation + forwarding core log events into the library logger.
 */
import * as addon from "ggstreamlog-node";

import { logger } from "../logging";

export { addon };

/**
 * One record handed to a host sink callback (mirrors the native `SinkRecord`).
 * `offset` is opaque — echo it back in `resolveOutcome`'s `failedOffsets` to re-deliver it.
 */
export interface SinkRecord {
  offset: number;
  partitionKey: string;
  timestampMs: number;
  payload: Buffer;
}

/** Outcome codes returned to the export engine via `resolveOutcome` (mirror the core `SendOutcome`). */
export const SINK_OUTCOME = {
  /** Whole batch stored — the engine commits past it. */
  ALL_ACKED: 0,
  /** Only the offsets in `failedOffsets` were not stored — they are re-delivered. */
  PARTIAL: 1,
  /** Whole batch failed (retryable) — re-delivered. */
  FAILED: 2,
} as const;

/**
 * The native sink-callback bridge surface (typed; napi's generated `SinkTsfn` alias is opaque).
 * A `callback`-sink stream's export thread invokes the registered callback with `(err, [batchId,
 * records])`; the host must eventually call `resolveOutcome(batchId, code, failedOffsets?)`.
 */
interface SinkBridge {
  registerSinkCallback(
    streamName: string,
    callback: (err: Error | null, arg: [number, SinkRecord[]]) => void,
  ): void;
  resolveOutcome(batchId: number, code: number, failedOffsets?: number[] | null): void;
}

const bridge = addon as unknown as SinkBridge;

/**
 * Register the host JS sink callback for the named `callback`-sink stream. Must be called **before**
 * {@link StreamService.open}. The callback receives `(err, [batchId, records])` and must call
 * {@link resolveSinkOutcome} with the same `batchId` once its (async) drain completes.
 */
export function registerSinkCallback(
  streamName: string,
  callback: (batchId: number, records: SinkRecord[]) => void,
): void {
  bridge.registerSinkCallback(streamName, (_err, arg) => {
    // The Rust tuple `(f64, Vec<SinkRecord>)` arrives as a 2-element JS array.
    callback(arg[0], arg[1]);
  });
}

/** Signal the blocked export thread that batch `batchId` finished (see {@link SINK_OUTCOME}). */
export function resolveSinkOutcome(batchId: number, code: number, failedOffsets?: number[]): void {
  bridge.resolveOutcome(batchId, code, failedOffsets ?? null);
}

/** Error thrown when a native streaming call fails. `code` mirrors `ggsl_status`. */
export class GgStreamError extends Error {
  constructor(
    readonly code: number,
    message?: string,
  ) {
    super(`ggstreamlog error ${code}${message ? `: ${message}` : ""}`);
    this.name = "GgStreamError";
  }
}

/** Native errors carry the message `ggsl:<code>:<message>`; parse it into a {@link GgStreamError}. */
export function translate(e: unknown): GgStreamError {
  const msg = e instanceof Error ? e.message : String(e);
  const m = /^ggsl:(\d+):([\s\S]*)$/.exec(msg);
  if (m) return new GgStreamError(parseInt(m[1], 10), m[2]);
  return new GgStreamError(-1, msg);
}

const LEVELS: Record<number, "error" | "warn" | "info" | "debug"> = {
  1: "error",
  2: "warn",
  3: "info",
  4: "debug",
  5: "debug",
};

let logForwardingInstalled = false;

/** Forward core log events into the library logger (idempotent; called on first service open). */
export function ensureLogForwarding(): void {
  if (logForwardingInstalled) return;
  logForwardingInstalled = true;
  try {
    addon.setLogCallback((_err: Error | null, ev: addon.LogEvent) => {
      try {
        logger[LEVELS[ev.level] ?? "debug"](`[${ev.target}] ${ev.message}`);
      } catch {
        /* never throw back into native */
      }
    });
  } catch {
    /* a logging-bridge failure must not break streaming */
  }
}
