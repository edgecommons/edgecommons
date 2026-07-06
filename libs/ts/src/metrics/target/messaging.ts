/**
 * Metrics target — messaging (TypeScript).
 *
 * Publishes each metric to the library-owned UNS metric topic
 * `ecv1[/{site}]/{device}/{component}/main/metric/{metricName}` (UNS-CANONICAL-DESIGN §4.3;
 * the metric name sanitized as a channel token) through the privileged reserved-publish seam —
 * the `metric` class is reserved (§4.1). Mirrors the Java `metrics.targets.Messaging`:
 *  - `emit` and `emitNow` both publish immediately (no batching).
 *  - For each EMF variant the object is wrapped in a `Message` envelope
 *    (`name = "Metric"`, `version = "1.0"`, body = EMF, identity + tags from the config-bound
 *    builder) and sent to the local transport or northbound broker per
 *    `metricEmission.targetConfig.destination` (D-U9; the legacy `targetConfig.topic` override
 *    is removed).
 *  - `largeFleetWorkaround` emits both the normal and the `coreName="ALL"` record.
 */
import type { MetricTarget } from "../types";
import type { MeasureValues } from "../types";
import type { Metric } from "../metric";
import { buildEmfVariants } from "../emf";
import type { Config } from "../../config/model";
import { sanitize } from "../../config/template";
import type { IMessagingService } from "../../messaging/types";
import { publishReservedVia } from "../../messaging/service";
import { MessageBuilder } from "../../message";
import { Uns, UnsClass } from "../../uns";

/** Publishes EMF metrics to UNS metric topics, wrapped in a `Metric` message envelope. */
export class MessagingMetricTarget implements MetricTarget {
  private readonly messaging: IMessagingService;
  private readonly config: Config;
  private readonly northbound: boolean;
  private readonly namespace: string;
  private readonly largeFleetWorkaround: boolean;
  private readonly uns: Uns;

  /**
   * Create the target. `northbound` selects the northbound broker over the local broker for the metric
   * envelopes; the topic is minted per metric from the config's resolved UNS identity.
   */
  constructor(
    messaging: IMessagingService,
    config: Config,
    northbound: boolean,
    namespace: string,
    largeFleetWorkaround: boolean,
  ) {
    this.messaging = messaging;
    this.config = config;
    this.northbound = northbound;
    this.namespace = namespace;
    this.largeFleetWorkaround = largeFleetWorkaround;
    this.uns = new Uns(config.componentIdentity, config.topicIncludeRoot);
  }

  /**
   * The metric's UNS topic — `ecv1[/{site}]/{device}/{component}/main/metric/{name}` with the
   * metric name passed through the template sanitizer (the §2.2 channel-token rule).
   */
  private metricTopic(metric: Metric): string {
    return this.uns.topic(UnsClass.Metric, sanitize(metric.getName()));
  }

  /** Wrap each EMF variant in a `Metric` envelope and publish it through the reserved seam. */
  private async publish(metric: Metric, values: MeasureValues): Promise<void> {
    const topic = this.metricTopic(metric);
    const variants = buildEmfVariants(this.namespace, metric, values, this.largeFleetWorkaround);
    for (const emf of variants) {
      const message = MessageBuilder.create("Metric", "1.0")
        .withPayload(emf)
        .withConfig(this.config)
        .build();
      // The metric class is reserved (§4.1) - publish through the privileged seam (§4.2).
      await publishReservedVia(this.messaging, topic, message, this.northbound ? "northbound" : "local");
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
