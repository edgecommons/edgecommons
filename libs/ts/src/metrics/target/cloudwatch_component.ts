/**
 * Metrics target — CloudWatch component (TypeScript).
 *
 * Publishes metrics to the Greengrass CloudWatch Metrics component over messaging.
 * Mirrors the Rust `metrics::target::cloudwatch_component::CloudWatchComponentTarget`
 * (and the Java/Python `cloudwatchcomponent` target):
 *  - `emit` and `emitNow` both publish immediately.
 *  - Wire format: one **raw** message is published *per measure*, shaped as the
 *    component's `PutMetricData` contract:
 *    `{ request: { namespace, metricData: { metricName, timestamp, value, unit,
 *      dimensions: [ { name, value } ] } } }`.
 *  - `timestamp` is epoch **seconds** (the component's contract — distinct from
 *    EMF's millisecond `_aws.Timestamp`).
 *  - Dimensions **exclude** `coreName` (the component supplies it implicitly).
 *  - Does **not** honor `largeFleetWorkaround` (the component owns `coreName`).
 *
 * Note (deviation from the prose spec): the spec said "wrap EMF in a Message
 * envelope and publish locally". The Rust reference instead publishes a *raw*
 * `{request:{...}}` payload per measure via `publishRaw`. Per "Match
 * cloudwatch_component.rs", this mirrors the Rust behavior, not the prose.
 */
import type { MetricTarget } from "../types";
import type { MeasureValues } from "../types";
import type { Metric } from "../metric";
import type { IMessagingService } from "../../messaging/types";

/** Publishes metrics to the Greengrass CloudWatch Metrics component topic. */
export class CloudWatchComponentTarget implements MetricTarget {
  private readonly messaging: IMessagingService;
  private readonly topic: string;
  private readonly namespace: string;

  constructor(messaging: IMessagingService, topic: string, namespace: string) {
    this.messaging = messaging;
    this.topic = topic;
    this.namespace = namespace;
  }

  /**
   * Build the `dimensions` array (`[{ name, value }, ...]`) excluding `coreName`,
   * matching Rust's `dimensions_array` / Java's `dimensionsAsJson(false)`.
   */
  private dimensionsArray(metric: Metric): Array<{ name: string; value: string }> {
    const dims: Array<{ name: string; value: string }> = [];
    for (const [key, value] of metric.getDimensions()) {
      if (key !== "coreName") {
        dims.push({ name: key, value });
      }
    }
    return dims;
  }

  /** Publish one `PutMetricData` request per measure value. */
  private async publish(metric: Metric, values: MeasureValues): Promise<void> {
    const timestamp = Math.floor(Date.now() / 1000); // epoch seconds
    const dimensions = this.dimensionsArray(metric);
    for (const [measureName, value] of Object.entries(values)) {
      const unit = metric.getMeasure(measureName)?.unit ?? "None";
      const payload = {
        request: {
          namespace: this.namespace,
          metricData: {
            metricName: measureName,
            timestamp,
            value,
            unit,
            dimensions,
          },
        },
      };
      await this.messaging.publishRaw(this.topic, payload);
    }
  }

  async emit(metric: Metric, values: MeasureValues): Promise<void> {
    await this.publish(metric, values);
  }

  async emitNow(metric: Metric, values: MeasureValues): Promise<void> {
    await this.publish(metric, values);
  }

  async flush(): Promise<void> {
    // No batching: nothing to flush.
  }

  async shutdown(): Promise<void> {
    // No resources to release.
  }
}
