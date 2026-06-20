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
