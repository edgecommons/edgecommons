/**
 * Metrics target — durable, store-and-forward CloudWatch (TypeScript).
 *
 * Gives the direct `cloudwatch` target a disk-backed buffer that survives lengthy cloud
 * disconnects, by reusing the shared `ggstreamlog` durable log + at-least-once export engine
 * through a **host-callback sink** (see `docs/CLOUDWATCH_DURABLE_METRICS.md`). The native export
 * thread blocks on this layer's async `PutMetricData` drain via the napi sink-callback bridge
 * ({@link registerSinkCallback} / {@link resolveSinkOutcome}).
 *
 * Flow:
 *  - `emit`/`emitNow` build `MetricDatum`s exactly as {@link CloudWatchTarget} does, then serialize
 *    each as a compact `{namespace, datum}` record (partition key = namespace) and `append` it to a
 *    `callback`-sink ggstreamlog stream instead of an in-memory queue. Memory stays flat under a
 *    disconnect; the backlog is disk-bounded (`onFull: dropOldest`).
 *  - On reconnect the export engine drains: the sink callback deserializes the batch, **groups by
 *    namespace**, **drops datums outside CloudWatch's ~2-weeks-past / ~2-hours-future accept window**
 *    (counted in {@link DurableCloudWatchTarget.droppedStale} — a stale timestamp can never be
 *    retried), chunks to <=1000 datums / <=~1 MB, and calls `PutMetricData` per chunk. Throttle/5xx/
 *    transport failures map to a retryable outcome so the engine re-delivers (the backlog persists);
 *    a successful send acks the batch and advances the checkpoint.
 *
 * The AWS SDK (`@aws-sdk/client-cloudwatch`) is an optional dependency, loaded lazily; absence is a
 * `GgError.metrics(...)` at {@link DurableCloudWatchTarget.create} (mirrors the in-memory target).
 */
import type { MetricTarget, MeasureValues } from "../types";
import type { Metric } from "../metric";
import { GgError } from "../../errors";
import {
  registerSinkCallback,
  resolveSinkOutcome,
  SINK_OUTCOME,
  type SinkRecord,
} from "../../streaming/native";
import { StreamService } from "../../streaming/service";
import { logger } from "../../logging";

/** Max datums per `PutMetricData` request (CloudWatch hard limit). */
const MAX_DATUMS_PER_REQUEST = 1000;
/** Max request body (CloudWatch limit ~1 MB); we chunk conservatively below it. */
const MAX_REQUEST_BYTES = 1_000_000;
/** CloudWatch accepts timestamps up to ~2 weeks in the past. */
const MAX_PAST_MS = 14 * 24 * 60 * 60 * 1000;
/** ...and up to ~2 hours in the future. */
const MAX_FUTURE_MS = 2 * 60 * 60 * 1000;

/** Minimal structural view of a CloudWatch `MetricDatum` (avoids an SDK type import). */
interface MetricDatum {
  MetricName: string;
  Value: number;
  Unit?: string;
  StorageResolution?: number;
  /** Epoch millis on the wire (record JSON); rehydrated to a `Date` before the SDK call. */
  Timestamp?: number;
  Dimensions?: Array<{ Name: string; Value: string }>;
}

/** Minimal structural view of the parts of the SDK client we use. */
interface CloudWatchClientLike {
  send(command: unknown): Promise<unknown>;
}

/** The bits of `@aws-sdk/client-cloudwatch` we depend on. */
interface CloudWatchModule {
  CloudWatchClient: new (config?: unknown) => CloudWatchClientLike;
  PutMetricDataCommand: new (input: {
    Namespace: string;
    MetricData: Array<Omit<MetricDatum, "Timestamp"> & { Timestamp?: Date }>;
  }) => unknown;
}

/** A serialized buffer record: one datum tagged with its namespace (partition key). */
interface NamespacedDatum {
  namespace: string;
  datum: MetricDatum;
}

/**
 * Whether an SDK error is transient (retry the whole batch) vs. a permanent reject (drop it so the
 * stream is not wedged forever). Throttling / 5xx / network errors are retryable; 4xx validation
 * errors are not. Mirrors the Rust/Java outcome mapping.
 */
function isRetryable(e: unknown): boolean {
  const err = e as { name?: string; $metadata?: { httpStatusCode?: number }; code?: string };
  const status = err?.$metadata?.httpStatusCode;
  if (typeof status === "number") {
    if (status === 429 || status >= 500) return true;
    if (status >= 400) return false; // 4xx (validation, throttling exceptions excepted below)
  }
  const name = err?.name ?? err?.code ?? "";
  if (/Throttl|TooManyRequests|RequestLimitExceeded|ServiceUnavailable|Timeout|ECONN|ETIMEDOUT|EAI_AGAIN/i.test(name)) {
    return true;
  }
  // Unknown shape: assume transient so we never silently drop on an ambiguous error.
  return status === undefined;
}

/** A durable, disk-backed CloudWatch target draining via the ggstreamlog export engine. */
export class DurableCloudWatchTarget implements MetricTarget {
  /** Count of datums dropped because their timestamp fell outside CloudWatch's accept window. */
  droppedStale = 0;

  private readonly module: CloudWatchModule;
  private readonly client: CloudWatchClientLike;
  private readonly namespace: string;
  private readonly largeFleetWorkaround: boolean;
  private readonly streamName: string;
  /** Set in {@link create} immediately after the stream is opened (definite assignment). */
  private svc!: StreamService;
  private closed = false;

  private constructor(
    module: CloudWatchModule,
    client: CloudWatchClientLike,
    namespace: string,
    largeFleetWorkaround: boolean,
    streamName: string,
  ) {
    this.module = module;
    this.client = client;
    this.namespace = namespace;
    this.largeFleetWorkaround = largeFleetWorkaround;
    this.streamName = streamName;
  }

  /**
   * Build the durable target: register the sink callback, then open a single `callback`-sink
   * ggstreamlog stream with the given buffer settings. The callback must be registered *before*
   * `StreamService.open` so the engine wires it in.
   *
   * @param buffer durable buffer settings (resolved `path`, `maxDiskBytes`, `onFull`, `fsync`).
   */
  static async create(
    namespace: string,
    largeFleetWorkaround: boolean,
    intervalSecs: number,
    buffer: {
      path: string;
      maxDiskBytes: number;
      onFull?: "dropOldest" | "block" | "rejectNew";
      fsync?: "perBatch" | "interval" | "always";
      segmentBytes?: number;
    },
    clientFactory?: () => CloudWatchClientLike,
  ): Promise<DurableCloudWatchTarget> {
    let module: CloudWatchModule;
    try {
      module = (await import("@aws-sdk/client-cloudwatch")) as unknown as CloudWatchModule;
    } catch {
      throw GgError.metrics(
        "metric target 'cloudwatch' (durable) requires the optional '@aws-sdk/client-cloudwatch' dependency",
      );
    }
    const client = clientFactory ? clientFactory() : new module.CloudWatchClient({});

    // A stable stream name keyed by namespace (one buffer per target; group-by-namespace in drain).
    const streamName = `metrics-cloudwatch`;
    const target = new DurableCloudWatchTarget(
      module,
      client,
      namespace,
      largeFleetWorkaround,
      streamName,
    );

    // Register BEFORE open so the export engine binds the host sink to the callback stream.
    registerSinkCallback(streamName, (batchId, records) => {
      // Fire-and-forget the async drain; it resolves the outcome when done. Errors are mapped to a
      // retryable outcome inside drain(); a throw here would never unblock the export thread, so we
      // guard with a catch that resolves FAILED.
      target.drain(batchId, records).catch((e) => {
        logger.error(`durable CloudWatch drain crashed; re-delivering batch: ${String(e)}`);
        resolveSinkOutcome(batchId, SINK_OUTCOME.FAILED);
      });
    });

    const segmentBytes = buffer.segmentBytes ?? 8 * 1024 * 1024;
    const config = JSON.stringify({
      streams: [
        {
          name: streamName,
          sink: { type: "callback" },
          buffer: {
            type: "disk",
            path: buffer.path,
            segmentBytes,
            maxDiskBytes: buffer.maxDiskBytes,
            onFull: buffer.onFull ?? "dropOldest",
            fsync: buffer.fsync ?? "perBatch",
          },
          // Flush a partial batch on the configured interval so low metric rates still drain.
          batch: { maxRecords: MAX_DATUMS_PER_REQUEST, maxBytes: MAX_REQUEST_BYTES, maxLatencyMs: Math.max(1, intervalSecs) * 1000 },
          delivery: { maxRetries: -1, pollIntervalMs: Math.min(1000, Math.max(1, intervalSecs) * 1000) },
        },
      ],
    });

    target.svc = StreamService.open(config);
    return target;
  }

  /** All datums for one emission (plus the `coreName="ALL"` set when `largeFleetWorkaround`). */
  private datumsFor(metric: Metric, values: MeasureValues): MetricDatum[] {
    const datums = this.toDatums(metric, values, false);
    if (this.largeFleetWorkaround) {
      datums.push(...this.toDatums(metric, values, true));
    }
    return datums;
  }

  /** Convert a metric + values into datums (one per measure value). */
  private toDatums(metric: Metric, values: MeasureValues, maskCoreName: boolean): MetricDatum[] {
    const dimensions: Array<{ Name: string; Value: string }> = [];
    for (const [key, value] of metric.getDimensions()) {
      const dimValue = maskCoreName && key === "coreName" ? "ALL" : value;
      dimensions.push({ Name: key, Value: dimValue });
    }
    const timestamp = Date.now();
    const datums: MetricDatum[] = [];
    for (const [measureName, value] of Object.entries(values)) {
      const measure = metric.getMeasure(measureName);
      datums.push({
        MetricName: measureName,
        Value: value,
        Unit: measure?.unit ?? "None",
        StorageResolution: measure?.storageResolution ?? 60,
        Timestamp: timestamp,
        Dimensions: dimensions.map((d) => ({ ...d })),
      });
    }
    return datums;
  }

  /** Append one emission's datums to the durable buffer (the namespace is the partition key). */
  private append(datums: MetricDatum[]): void {
    if (this.closed) return;
    const handle = this.svc.stream(this.streamName);
    for (const datum of datums) {
      const record: NamespacedDatum = { namespace: this.namespace, datum };
      const payload = Buffer.from(JSON.stringify(record), "utf8");
      handle.append(this.namespace, datum.Timestamp ?? Date.now(), payload);
    }
  }

  /**
   * The host sink callback: drain one export batch. Deserialize -> group by namespace -> drop stale
   * -> chunk -> `PutMetricData` -> map to an outcome -> {@link resolveSinkOutcome}.
   *
   * Outcome semantics (at-least-once): a fully-sent batch acks (`ALL_ACKED`); a retryable transport
   * failure on any chunk re-delivers the *whole* batch (`FAILED`) — duplicates on CloudWatch are
   * cheap and idempotent enough vs. losing data. Permanent per-chunk rejects are logged and dropped.
   */
  private async drain(batchId: number, records: SinkRecord[]): Promise<void> {
    const now = Date.now();
    // Group surviving datums by namespace; drop stale ones (counted, never sent or retried).
    const byNamespace = new Map<string, MetricDatum[]>();
    for (const rec of records) {
      let parsed: NamespacedDatum;
      try {
        parsed = JSON.parse(rec.payload.toString("utf8")) as NamespacedDatum;
      } catch {
        // A corrupt record can never succeed; count it as dropped and skip (ack it).
        this.droppedStale++;
        continue;
      }
      const ts = parsed.datum.Timestamp ?? rec.timestampMs;
      if (ts < now - MAX_PAST_MS || ts > now + MAX_FUTURE_MS) {
        this.droppedStale++;
        continue;
      }
      const ns = parsed.namespace || this.namespace;
      let list = byNamespace.get(ns);
      if (list === undefined) {
        list = [];
        byNamespace.set(ns, list);
      }
      list.push(parsed.datum);
    }

    // Send every namespace's datums in <=1000 / <=~1MB chunks. Any retryable failure aborts and
    // re-delivers the whole batch; a permanent reject is dropped (logged).
    try {
      for (const [namespace, datums] of byNamespace) {
        for (const chunk of this.chunk(datums)) {
          await this.putChunk(namespace, chunk);
        }
      }
      resolveSinkOutcome(batchId, SINK_OUTCOME.ALL_ACKED);
    } catch (e) {
      if (isRetryable(e)) {
        logger.warn(`PutMetricData transient failure; re-delivering batch (backlog persists): ${String(e)}`);
        resolveSinkOutcome(batchId, SINK_OUTCOME.FAILED);
      } else {
        logger.error(`PutMetricData permanent reject; dropping batch to unwedge stream: ${String(e)}`);
        resolveSinkOutcome(batchId, SINK_OUTCOME.ALL_ACKED);
      }
    }
  }

  /** Send one chunk for one namespace; rehydrate the epoch-millis timestamp to a `Date`. */
  private async putChunk(namespace: string, chunk: MetricDatum[]): Promise<void> {
    const MetricData = chunk.map((d) => ({
      MetricName: d.MetricName,
      Value: d.Value,
      Unit: d.Unit,
      StorageResolution: d.StorageResolution,
      Timestamp: d.Timestamp !== undefined ? new Date(d.Timestamp) : undefined,
      Dimensions: d.Dimensions,
    }));
    const command = new this.module.PutMetricDataCommand({ Namespace: namespace, MetricData });
    await this.client.send(command);
  }

  /** Split datums into <=1000-item and <=~1MB chunks (whichever bound hits first). */
  private *chunk(datums: MetricDatum[]): Generator<MetricDatum[]> {
    let cur: MetricDatum[] = [];
    let curBytes = 0;
    for (const d of datums) {
      const sz = JSON.stringify(d).length;
      if (cur.length > 0 && (cur.length >= MAX_DATUMS_PER_REQUEST || curBytes + sz > MAX_REQUEST_BYTES)) {
        yield cur;
        cur = [];
        curBytes = 0;
      }
      cur.push(d);
      curBytes += sz;
    }
    if (cur.length > 0) yield cur;
  }

  async emit(metric: Metric, values: MeasureValues): Promise<void> {
    this.append(this.datumsFor(metric, values));
  }

  async emitNow(metric: Metric, values: MeasureValues): Promise<void> {
    // Durable target: emitNow also appends (the engine drains continuously). Force fsync so the
    // record survives an immediate crash.
    this.append(this.datumsFor(metric, values));
    if (!this.closed) this.svc.stream(this.streamName).flush();
  }

  async flush(): Promise<void> {
    // Force the buffer to disk; the export engine drains it asynchronously.
    if (!this.closed) this.svc.stream(this.streamName).flush();
  }

  async shutdown(): Promise<void> {
    if (this.closed) return;
    this.closed = true;
    // Per the design (section 5): flush to disk + stop the engine; do NOT block on a cloud drain.
    // The backlog persists and resumes on the next start.
    this.svc.close();
  }
}
