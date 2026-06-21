/**
 * High-level TypeScript API for the `ggstreamlog` telemetry-streaming core, over the napi addon.
 * Gives Node components the same durable store-and-forward streaming + config schema as the Rust,
 * Java, and Python libraries. Mirrors `gg.streams()`.
 */
import { addon, ensureLogForwarding, GgStreamError, translate } from "./native";

/** A snapshot of one stream's buffer + export progress (mirrors `ggsl_stats_t`). */
export interface StreamStats {
  appendedTotal: number;
  exportedTotal: number;
  droppedTotal: number;
  retriesTotal: number;
  failedTotal: number;
  backlog: number;
  diskBytes: number;
  ackedOffset: number;
  nextOffset: number;
  oldestUnackedAgeMs: number;
}

/** A producer handle to one telemetry stream. */
export class StreamHandle {
  constructor(
    private inner: addon.StreamHandle | null,
    readonly name: string,
  ) {}

  /** Append one record; returns once durable per the stream's fsync policy. */
  append(partitionKey: string, timestampMs: number, payload: Buffer | Uint8Array): void {
    if (!this.inner) throw new Error("StreamHandle is closed");
    const buf = Buffer.isBuffer(payload) ? payload : Buffer.from(payload);
    try {
      this.inner.append(partitionKey, timestampMs, buf);
    } catch (e) {
      throw translate(e);
    }
  }

  /** Force this stream's buffer durably to disk (does not wait for export). */
  flush(): void {
    if (!this.inner) throw new Error("StreamHandle is closed");
    try {
      this.inner.flush();
    } catch (e) {
      throw translate(e);
    }
  }

  /** Release the handle (the native buffer ref is dropped by GC). Idempotent. */
  close(): void {
    this.inner = null;
  }
}

/** Owns the native streaming service: opens streams from config, runs export, hands out handles. */
export class StreamService {
  constructor(private inner: addon.StreamService | null) {}

  /** Open every stream in `configJson` (the `streaming` section; templates pre-resolved). */
  static open(configJson: string): StreamService {
    ensureLogForwarding();
    try {
      return new StreamService(addon.StreamService.open(configJson));
    } catch (e) {
      throw translate(e);
    }
  }

  /** A handle to the named stream (throws `GgStreamError` ERR_UNKNOWN_STREAM if not configured). */
  stream(name: string): StreamHandle {
    try {
      return new StreamHandle(this.require().stream(name), name);
    } catch (e) {
      throw translate(e);
    }
  }

  /** A stats snapshot for the named stream (throws ERR_UNKNOWN_STREAM if not configured). */
  stats(name: string): StreamStats {
    try {
      return this.require().stats(name);
    } catch (e) {
      throw translate(e);
    }
  }

  /** The stream names declared in a `streaming` config document (empty if none/invalid). */
  static streamNames(configJson: string): string[] {
    try {
      const doc = JSON.parse(configJson) as { streams?: Array<{ name?: string }> };
      if (!doc || !Array.isArray(doc.streams)) return [];
      return doc.streams.filter((s) => typeof s?.name === "string").map((s) => s.name as string);
    } catch {
      return [];
    }
  }

  /** Flush every buffer, stop the export engines, and free the service. Idempotent. */
  close(): void {
    if (this.inner) {
      this.inner.close();
      this.inner = null;
    }
  }

  private require(): addon.StreamService {
    if (!this.inner) throw new Error("StreamService is closed");
    return this.inner;
  }
}

export { GgStreamError };
