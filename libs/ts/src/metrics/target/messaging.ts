/**
 * Metrics target — messaging (TypeScript).
 *
 * Publishes EMF metrics over the messaging service (local broker or AWS IoT Core).
 * Mirrors the Rust `metrics::target::messaging::MessagingMetricTarget` (and the
 * Java/Python `messaging` target):
 *  - `emit` and `emitNow` both publish immediately (no batching).
 *  - For each EMF variant the object is wrapped in a `Message` envelope
 *    (`name = "Metric"`, `version = "1.0"`, body = EMF, tags = thing name + the
 *    configured tags) and sent via `publish` (local) or `publishToIotCore`
 *    (`Qos.AtLeastOnce`) when `iotCore` is set.
 *  - `largeFleetWorkaround` emits both the normal and the `coreName="ALL"` record.
 */
import type { MetricTarget } from "../types";
import type { MeasureValues } from "../types";
import type { Metric } from "../metric";
import { buildEmfVariants } from "../emf";
import type { IMessagingService } from "../../messaging/types";
import { Qos } from "../../messaging/types";
import { MessageBuilder } from "../../message";

/** Publishes EMF metrics over messaging, wrapped in a `Metric` message envelope. */
export class MessagingMetricTarget implements MetricTarget {
  private readonly messaging: IMessagingService;
  private readonly topic: string;
  private readonly iotCore: boolean;
  private readonly namespace: string;
  private readonly largeFleetWorkaround: boolean;
  private readonly thingName: string;
  private readonly tags: Record<string, unknown>;

  /**
   * Create the target. `iotCore` selects AWS IoT Core over the local broker.
   * `thingName` and `tags` populate the message envelope (mirroring `withConfig`).
   */
  constructor(
    messaging: IMessagingService,
    topic: string,
    iotCore: boolean,
    namespace: string,
    largeFleetWorkaround: boolean,
    thingName: string,
    tags: Record<string, unknown>,
  ) {
    this.messaging = messaging;
    this.topic = topic;
    this.iotCore = iotCore;
    this.namespace = namespace;
    this.largeFleetWorkaround = largeFleetWorkaround;
    this.thingName = thingName;
    this.tags = tags;
  }

  /** Wrap each EMF variant in a `Metric` envelope and publish it. */
  private async publish(metric: Metric, values: MeasureValues): Promise<void> {
    const variants = buildEmfVariants(this.namespace, metric, values, this.largeFleetWorkaround);
    for (const emf of variants) {
      let builder = MessageBuilder.create("Metric", "1.0").withThingName(this.thingName).withPayload(emf);
      for (const [key, value] of Object.entries(this.tags)) {
        builder = builder.withTag(key, value);
      }
      const message = builder.build();
      if (this.iotCore) {
        await this.messaging.publishToIotCore(this.topic, message, Qos.AtLeastOnce);
      } else {
        await this.messaging.publish(this.topic, message);
      }
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
