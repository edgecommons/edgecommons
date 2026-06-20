/**
 * Metrics — CloudWatch Embedded Metric Format (EMF).
 *
 * Flattens dimension values and measure values to the top level and attaches the
 * `_aws` metadata block (single dimension set = all dimension keys), matching the
 * Java/Python/Rust EMF layout. `_aws.Timestamp` is in **milliseconds** since the
 * Unix epoch per the official EMF spec. `largeFleetWorkaround` masks `coreName` to
 * `"ALL"`.
 */
import type { MeasureValues } from "./types";
import type { Metric } from "./metric";

/** Build an EMF JSON object for `metric` with the given measure values. */
export function buildEmf(
  namespace: string,
  metric: Metric,
  measureValues: MeasureValues,
  largeFleetWorkaround: boolean,
): Record<string, unknown> {
  const root: Record<string, unknown> = {};

  for (const [key, value] of metric.getDimensions()) {
    root[key] = largeFleetWorkaround && key === "coreName" ? "ALL" : value;
  }
  for (const [key, value] of Object.entries(measureValues)) {
    root[key] = value;
  }
  root._aws = metricsMetadata(namespace, metric);
  return root;
}

/**
 * The EMF objects to emit for one emission: the normal object, plus a second
 * `coreName="ALL"` duplicate when `largeFleetWorkaround` is set (matching the other
 * libraries, which emit both records).
 */
export function buildEmfVariants(
  namespace: string,
  metric: Metric,
  measureValues: MeasureValues,
  largeFleetWorkaround: boolean,
): Array<Record<string, unknown>> {
  const variants = [buildEmf(namespace, metric, measureValues, false)];
  if (largeFleetWorkaround) {
    variants.push(buildEmf(namespace, metric, measureValues, true));
  }
  return variants;
}

function metricsMetadata(namespace: string, metric: Metric): Record<string, unknown> {
  const dimensionKeys = [...metric.getDimensions().keys()];
  const measures = [...metric.getMeasures().values()].map((m) => ({
    Name: m.name,
    Unit: m.unit,
    StorageResolution: m.storageResolution,
  }));
  return {
    Timestamp: Date.now(),
    CloudWatchMetrics: [
      {
        Namespace: namespace,
        Dimensions: [dimensionKeys],
        Metrics: measures,
      },
    ],
  };
}
