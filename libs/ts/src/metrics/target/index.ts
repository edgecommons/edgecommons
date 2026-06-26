/**
 * Metrics targets — public surface.
 *
 * Re-exports the {@link MetricTarget} interface (defined in `../types`) and its
 * concrete implementations, mirroring the Rust `metrics::target` module.
 */
export type { MetricTarget } from "../types";
export { LogTarget } from "./log";
export { MessagingMetricTarget } from "./messaging";
export { CloudWatchComponentTarget } from "./cloudwatch_component";
export { CloudWatchTarget } from "./cloudwatch";
export { DurableCloudWatchTarget } from "./cloudwatch_durable";
// The prometheus target's `prom-client` dependency is loaded lazily inside `create`, so re-exporting
// the class here does not eagerly pull in the optional dependency.
export { PrometheusTarget, sanitizeMetricName, sanitizeLabelName } from "./prometheus";
