/**
 * Metrics subsystem — public surface.
 *
 * Re-exports the metric model, EMF builders, service/target interfaces, the default
 * {@link MetricEmitter}, and the concrete targets. Mirrors the Rust `metrics` module.
 */
export { Metric, Measure, MetricBuilder } from "./metric";
export { buildEmf, buildEmfVariants } from "./emf";
export type { MetricService, MetricTarget, MeasureValues } from "./types";
export { MetricEmitter } from "./service";
export * from "./target";
