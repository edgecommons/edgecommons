/**
 * Metrics — service + target interfaces (mirrors the Java/Python `IMetricService`
 * and the Rust `MetricService` / `MetricTarget` traits).
 */
import type { Metric } from "./metric";

/** A flat map of measure name → value. */
export type MeasureValues = Record<string, number>;

/** Define and emit metrics through a configured target. */
export interface MetricService {
  /** Register a metric definition by name (replacing any prior definition). */
  defineMetric(metric: Metric): void;
  /** Whether a metric with `name` has been defined. */
  isMetricDefined(name: string): boolean;
  /** Emit measure values for a defined metric (buffered where the target batches). */
  emitMetric(name: string, values: MeasureValues): Promise<void>;
  /** Emit measure values immediately, bypassing batching. */
  emitMetricNow(name: string, values: MeasureValues): Promise<void>;
  /** Flush any buffered metrics. */
  flushMetrics(): Promise<void>;
  /** Release resources / final flush. */
  shutdown(): Promise<void>;
}

/** A destination metrics are emitted to (log file, messaging, CloudWatch, ...). */
export interface MetricTarget {
  /** Emit a metric's values (buffered where the target batches). */
  emit(metric: Metric, values: MeasureValues): Promise<void>;
  /** Emit immediately, bypassing batching. */
  emitNow(metric: Metric, values: MeasureValues): Promise<void>;
  /** Flush any buffered metrics (default: no-op). */
  flush(): Promise<void>;
  /** Release resources / final flush (default: no-op). */
  shutdown(): Promise<void>;
}
