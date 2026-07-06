/**
 * Bridges the `streamlog-node` napi addon (the shared Rust core, built as a native Node addon)
 * into the library: error translation + forwarding core log events into the library logger.
 */
import type * as Addon from "@edgecommons/streamlog-node";

import { logger } from "../logging";

/** The native addon's types (type-only; erased at runtime). */
export type { Addon };

let cachedAddon: typeof Addon | undefined;

/**
 * Lazily load the native `streamlog-node` addon. Importing this module does **not** load the native
 * library — it is required only on first actual use, so a component that imports edgecommons but never
 * uses streaming (e.g. a messaging-only component, or the interop node) never needs the addon present
 * (CLAUDE.md: "components that don't use it never load the native library").
 */
export function getAddon(): typeof Addon {
  return (cachedAddon ??= require("@edgecommons/streamlog-node") as typeof Addon);
}

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

/**
 * Register the host JS sink callback for the named `callback`-sink stream. Must be called **before**
 * {@link StreamService.open}. The callback receives `(err, [batchId, records])` and must call
 * {@link resolveSinkOutcome} with the same `batchId` once its (async) drain completes.
 */
export function registerSinkCallback(
  streamName: string,
  callback: (batchId: number, records: SinkRecord[]) => void,
): void {
  const bridge = getAddon() as unknown as SinkBridge;
  bridge.registerSinkCallback(streamName, (_err, arg) => {
    // The Rust tuple `(f64, Vec<SinkRecord>)` arrives as a 2-element JS array.
    callback(arg[0], arg[1]);
  });
}

/** Signal the blocked export thread that batch `batchId` finished (see {@link SINK_OUTCOME}). */
export function resolveSinkOutcome(batchId: number, code: number, failedOffsets?: number[]): void {
  (getAddon() as unknown as SinkBridge).resolveOutcome(batchId, code, failedOffsets ?? null);
}

/** Error thrown when a native streaming call fails. `code` mirrors `esl_status`. */
export class EdgeStreamError extends Error {
  constructor(
    readonly code: number,
    message?: string,
  ) {
    super(`edgestreamlog error ${code}${message ? `: ${message}` : ""}`);
    this.name = "EdgeStreamError";
  }
}

/** Native errors carry the message `esl:<code>:<message>`; parse it into a {@link EdgeStreamError}. */
export function translate(e: unknown): EdgeStreamError {
  const msg = e instanceof Error ? e.message : String(e);
  const m = /^esl:(\d+):([\s\S]*)$/.exec(msg);
  if (m) return new EdgeStreamError(parseInt(m[1], 10), m[2]);
  return new EdgeStreamError(-1, msg);
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
    getAddon().setLogCallback((_err: Error | null, ev: Addon.LogEvent) => {
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
