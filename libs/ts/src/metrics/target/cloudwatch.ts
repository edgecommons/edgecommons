/**
 * Metrics target — CloudWatch via AWS SDK (TypeScript).
 *
 * Sends metrics directly to Amazon CloudWatch via `PutMetricData`. Mirrors the Rust
 * `metrics::target::cloudwatch::CloudWatchTarget` (feature `cloudwatch`) and the
 * Java/Python `cloudwatch` target:
 *  - `emit` buffers `MetricDatum`s and a background timer flushes on the configured
 *    interval; `emitNow` and `flush` send immediately.
 *  - Each measure value becomes one datum (metric name = measure name) carrying the
 *    metric's dimensions. `largeFleetWorkaround` adds a second `coreName="ALL"` set.
 *  - Buffer flushes in chunks of <=1000 datums (the `PutMetricData` limit); per-chunk
 *    failures are logged (not propagated), others still sent.
 *  - The background flush timer is cleared on `shutdown`.
 *
 * The AWS SDK is an **optional** dependency, loaded lazily via dynamic import. If
 * `@aws-sdk/client-cloudwatch` is not installed, {@link CloudWatchTarget.create}
 * throws `EdgeCommonsError.metrics(...)` (mirroring Rust's `cloudwatch` feature gate).
 */
import type { MetricTarget } from "../types";
import type { MeasureValues } from "../types";
import type { Metric } from "../metric";
import { EdgeCommonsError } from "../../errors";

/** Max datums per `PutMetricData` request. */
const MAX_DATUMS_PER_REQUEST = 1000;

/** Minimal structural view of a CloudWatch `MetricDatum` (avoids an SDK type import). */
interface MetricDatum {
  MetricName: string;
  Value: number;
  Unit?: string;
  StorageResolution?: number;
  Timestamp?: Date;
  Dimensions?: Array<{ Name: string; Value: string }>;
}

/** Minimal structural view of the parts of the SDK client we use. */
interface CloudWatchClientLike {
  send(command: unknown): Promise<unknown>;
}

/** The bits of `@aws-sdk/client-cloudwatch` we depend on. */
interface CloudWatchModule {
  CloudWatchClient: new (config?: unknown) => CloudWatchClientLike;
  PutMetricDataCommand: new (input: { Namespace: string; MetricData: MetricDatum[] }) => unknown;
}

/** Sends metrics to CloudWatch via the AWS SDK. */
export class CloudWatchTarget implements MetricTarget {
  private readonly module: CloudWatchModule;
  private readonly client: CloudWatchClientLike;
  private readonly namespace: string;
  private readonly largeFleetWorkaround: boolean;
  private pending: MetricDatum[] = [];
  private flushTimer?: ReturnType<typeof setInterval>;

  private constructor(
    module: CloudWatchModule,
    client: CloudWatchClientLike,
    namespace: string,
    largeFleetWorkaround: boolean,
    intervalSecs: number,
  ) {
    this.module = module;
    this.client = client;
    this.namespace = namespace;
    this.largeFleetWorkaround = largeFleetWorkaround;

    const periodMs = Math.max(1, intervalSecs) * 1000;
    // Unlike Rust's tokio interval (which fires once immediately and is consumed),
    // setInterval first fires after one full period — already the desired cadence,
    // so nothing is flushed at startup (parity with Java/Python/Rust).
    this.flushTimer = setInterval(() => {
      void this.flushInternal();
    }, periodMs);
    // Do not keep the event loop alive solely for the flush timer.
    if (typeof this.flushTimer.unref === "function") {
      this.flushTimer.unref();
    }
  }

  /**
   * Build the target, lazily importing the optional AWS SDK and creating the client.
   * Throws `EdgeCommonsError.metrics(...)` if `@aws-sdk/client-cloudwatch` is not installed,
   * mirroring Rust's error when the `cloudwatch` feature is disabled.
   */
  static async create(
    namespace: string,
    largeFleetWorkaround: boolean,
    intervalSecs: number,
  ): Promise<CloudWatchTarget> {
    let module: CloudWatchModule;
    try {
      // Literal dynamic import: still loaded lazily (so the "absent -> EdgeCommonsError" path
      // is preserved) but the literal specifier lets vitest's `vi.mock` intercept it
      // for the batching/datum coverage.
      // TESTABILITY SEAM: was an indirect `import(pkg)` to keep tsc from resolving the
      // optional dep; a devDependency now provides it and the literal form is mockable.
      module = (await import("@aws-sdk/client-cloudwatch")) as unknown as CloudWatchModule;
    } catch {
      throw EdgeCommonsError.metrics(
        "metric target 'cloudwatch' requires the optional '@aws-sdk/client-cloudwatch' dependency",
      );
    }
    const client = new module.CloudWatchClient({});
    return new CloudWatchTarget(module, client, namespace, largeFleetWorkaround, intervalSecs);
  }

  /**
   * All datums for one emission: the normal datums plus a `coreName="ALL"` set when
   * `largeFleetWorkaround` is enabled. Matches Rust `datums_for`.
   */
  private datumsFor(metric: Metric, values: MeasureValues): MetricDatum[] {
    const datums = this.toDatums(metric, values, false);
    if (this.largeFleetWorkaround) {
      datums.push(...this.toDatums(metric, values, true));
    }
    return datums;
  }

  /** Convert a metric + values into datums (one per measure value). Matches `to_datums`. */
  private toDatums(metric: Metric, values: MeasureValues, maskCoreName: boolean): MetricDatum[] {
    const dimensions: Array<{ Name: string; Value: string }> = [];
    for (const [key, value] of metric.getDimensions()) {
      const dimValue = maskCoreName && key === "coreName" ? "ALL" : value;
      dimensions.push({ Name: key, Value: dimValue });
    }
    const timestamp = new Date(Date.now());

    const datums: MetricDatum[] = [];
    for (const [measureName, value] of Object.entries(values)) {
      const measure = metric.getMeasure(measureName);
      const unit = measure?.unit ?? "None";
      const resolution = measure?.storageResolution ?? 60;
      datums.push({
        MetricName: measureName,
        Value: value,
        Unit: unit,
        StorageResolution: resolution,
        Timestamp: timestamp,
        Dimensions: dimensions.map((d) => ({ ...d })),
      });
    }
    return datums;
  }

  /** Send datums in <=1000-item batches; log (don't propagate) per-batch failures. */
  private async sendBatches(datums: MetricDatum[]): Promise<void> {
    for (let i = 0; i < datums.length; i += MAX_DATUMS_PER_REQUEST) {
      const chunk = datums.slice(i, i + MAX_DATUMS_PER_REQUEST);
      const command = new this.module.PutMetricDataCommand({
        Namespace: this.namespace,
        MetricData: chunk,
      });
      try {
        await this.client.send(command);
      } catch (e) {
        // eslint-disable-next-line no-console
        console.error(`PutMetricData failed; dropping batch of ${chunk.length}: ${String(e)}`);
      }
    }
  }

  /** Drain the pending buffer and send it. */
  private async flushInternal(): Promise<void> {
    if (this.pending.length === 0) {
      return;
    }
    const batch = this.pending;
    this.pending = [];
    await this.sendBatches(batch);
  }

  async emit(metric: Metric, values: MeasureValues): Promise<void> {
    this.pending.push(...this.datumsFor(metric, values));
  }

  async emitNow(metric: Metric, values: MeasureValues): Promise<void> {
    await this.sendBatches(this.datumsFor(metric, values));
  }

  async flush(): Promise<void> {
    await this.flushInternal();
  }

  async shutdown(): Promise<void> {
    if (this.flushTimer !== undefined) {
      clearInterval(this.flushTimer);
      this.flushTimer = undefined;
    }
    await this.flush();
  }
}
