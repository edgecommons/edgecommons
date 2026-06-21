/**
 * Bridges the `ggstreamlog-node` napi addon (the shared Rust core, built as a native Node addon)
 * into the library: error translation + forwarding core log events into the library logger.
 */
import * as addon from "ggstreamlog-node";

import { logger } from "../logging";

export { addon };

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
