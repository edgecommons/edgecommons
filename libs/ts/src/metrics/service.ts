/**
 * Metrics — service (TypeScript).
 *
 * {@link MetricEmitter} routes metric emissions to the configured {@link MetricTarget},
 * the default {@link MetricService} implementation. Mirrors the Rust `MetricEmitter`:
 *  - `create` (Rust's `new`) selects the target via {@link buildTarget} from
 *    `config.parsed.metricEmission.target()`.
 *  - `defineMetric`/`isMetricDefined` back a name→{@link Metric} registry.
 *  - `emitMetric`/`emitMetricNow` look the metric up by name; an undefined metric is
 *    a warn-and-ignore no-op (not an error), matching Rust.
 *  - `flushMetrics`/`shutdown` delegate to the current target.
 *  - As a {@link ConfigurationChangeListener}, `onConfigurationChange` rebuilds the
 *    target (keeping the previous one on error), so the target is swappable.
 */
import type { MetricService, MeasureValues } from "./types";
import type { MetricTarget } from "./types";
import { Metric } from "./metric";
import { LogTarget } from "./target/log";
import { MessagingMetricTarget } from "./target/messaging";
import { CloudWatchComponentTarget } from "./target/cloudwatch_component";
import { CloudWatchTarget } from "./target/cloudwatch";
import { Config } from "../config/model";
import { resolve } from "../config/template";
import type { ConfigurationChangeListener } from "../config";
import type { IMessagingService } from "../messaging/types";
import { GgError } from "../errors";

/** Require a messaging service for targets that need one. Matches Rust `require_messaging`. */
function requireMessaging(
  messaging: IMessagingService | undefined,
  target: string,
): IMessagingService {
  if (messaging === undefined) {
    throw GgError.metrics(`metric target '${target}' requires a messaging service`);
  }
  return messaging;
}

/**
 * Build the configured metric target. Mirrors Rust `build_target`: selects by the effective target
 * (lower-cased); unknown targets warn and default to log.
 *
 * Effective-target precedence (FR-MET-4/FR-RT-3): explicit `metricEmission.target` config ▸
 * `targetDefault` (the platform-profile default — `prometheus` on KUBERNETES) ▸ library default `log`.
 * `targetDefault` is threaded from the resolved platform exactly like the logging-format default; it is
 * `undefined` for GREENGRASS/HOST so their behavior is unchanged.
 */
async function buildTarget(
  config: Config,
  messaging: IMessagingService | undefined,
  targetDefault?: string,
): Promise<MetricTarget> {
  const mc = config.parsed.metricEmission;
  const namespace = mc.namespace();
  const largeFleet = mc.largeFleetWorkaround;
  const targetName = (mc.explicitTarget() ?? targetDefault ?? "log").toLowerCase();

  const logTarget = (): MetricTarget =>
    new LogTarget(resolve(config, mc.logFileName()), namespace, largeFleet, mc.maxFileSize());

  switch (targetName) {
    case "log":
      return logTarget();
    case "messaging": {
      const svc = requireMessaging(messaging, "messaging");
      const topic = resolve(config, mc.topic());
      // Canonical "iot_core" (schema) plus legacy "iotcore" both select IoT Core;
      // everything else (e.g. "ipc"/"local") is the local transport.
      const dest = mc.destination().toLowerCase();
      const iotCore = dest === "iot_core" || dest === "iotcore";
      return new MessagingMetricTarget(
        svc,
        topic,
        iotCore,
        namespace,
        largeFleet,
        config.thingName,
        config.parsed.tags,
      );
    }
    case "cloudwatchcomponent": {
      const svc = requireMessaging(messaging, "cloudwatchcomponent");
      const topic = resolve(config, mc.topic());
      return new CloudWatchComponentTarget(svc, topic, namespace);
    }
    case "cloudwatch": {
      // The cloudwatch target defaults to a durable (disk-backed) store-and-forward buffer that
      // survives lengthy cloud disconnects; `targetConfig.cloudwatch.buffer.type: memory` opts back
      // into the legacy in-memory batching target.
      const buffer = mc.cloudwatchBuffer();
      if (buffer !== undefined && buffer.type === "memory") {
        return CloudWatchTarget.create(namespace, largeFleet, mc.intervalSecs());
      }
      const buf = buffer ?? {
        type: "durable" as const,
        path: "/var/lib/ggcommons/metrics/{ComponentName}/cw",
        maxDiskBytes: 128 * 1024 * 1024,
        onFull: "dropOldest" as const,
        fsync: "perBatch" as const,
      };
      // Lazy-load the durable target so merely importing the metrics service never pulls in the
      // native ggstreamlog addon (CLAUDE.md: load the native library only when streaming is used).
      const { DurableCloudWatchTarget } = await import("./target/cloudwatch_durable");
      return DurableCloudWatchTarget.create(namespace, largeFleet, mc.intervalSecs(), {
        path: resolve(config, buf.path),
        maxDiskBytes: buf.maxDiskBytes,
        onFull: buf.onFull,
        fsync: buf.fsync,
      });
    }
    case "prometheus": {
      // Pull-based target: maintain an in-process registry served as OpenMetrics text over HTTP
      // (FR-MET-1). Lazy-import prom-client (optional dependency) only when actually selected, mirroring
      // the cloudwatch-durable lazy import — merely importing the metrics service never pulls it in.
      const { PrometheusTarget } = await import("./target/prometheus");
      return PrometheusTarget.create(namespace, mc.prometheusPort(), mc.prometheusPath());
    }
    default:
      // eslint-disable-next-line no-console
      console.warn(`unknown metric target '${targetName}'; defaulting to 'log'`);
      return logTarget();
  }
}

/** Routes metric emissions to the configured {@link MetricTarget}. */
export class MetricEmitter implements MetricService, ConfigurationChangeListener {
  private target: MetricTarget;
  private readonly metrics = new Map<string, Metric>();
  /** Retained so the target can be rebuilt on config hot-reload. */
  private readonly messaging?: IMessagingService;
  /** The platform-profile default target (e.g. `prometheus` on KUBERNETES); retained for hot-reload. */
  private readonly targetDefault?: string;

  private constructor(
    target: MetricTarget,
    messaging: IMessagingService | undefined,
    targetDefault: string | undefined,
  ) {
    this.target = target;
    this.messaging = messaging;
    this.targetDefault = targetDefault;
  }

  /**
   * Build an emitter from configuration, selecting the target (Rust's `new`).
   * `messaging` is required by the `messaging`/`cloudwatchcomponent` targets and is
   * retained so the target can be rebuilt on config change. `targetDefault` is the platform-profile
   * default metric target (e.g. `prometheus` on KUBERNETES, `undefined` elsewhere); it is consulted
   * only when the config sets no explicit `metricEmission.target` (precedence FR-MET-4/FR-RT-3).
   */
  static async create(
    config: Config,
    messaging?: IMessagingService,
    targetDefault?: string,
  ): Promise<MetricEmitter> {
    const target = await buildTarget(config, messaging, targetDefault);
    return new MetricEmitter(target, messaging, targetDefault);
  }

  defineMetric(metric: Metric): void {
    this.metrics.set(metric.getName(), metric);
  }

  isMetricDefined(name: string): boolean {
    return this.metrics.has(name);
  }

  async emitMetric(name: string, values: MeasureValues): Promise<void> {
    const metric = this.metrics.get(name);
    if (metric === undefined) {
      // eslint-disable-next-line no-console
      console.warn(`metric '${name}' is not defined; ignoring emit`);
      return;
    }
    await this.target.emit(metric, values);
  }

  async emitMetricNow(name: string, values: MeasureValues): Promise<void> {
    const metric = this.metrics.get(name);
    if (metric === undefined) {
      // eslint-disable-next-line no-console
      console.warn(`metric '${name}' is not defined; ignoring emit`);
      return;
    }
    await this.target.emitNow(metric, values);
  }

  async flushMetrics(): Promise<void> {
    await this.target.flush();
  }

  async shutdown(): Promise<void> {
    await this.target.shutdown();
  }

  /**
   * Rebuild the metric target from the new config (keeping the previous one on error). The
   * platform-profile default is preserved across reloads, so the precedence still holds. On a
   * successful swap the previous target is shut down so a pull-based target's HTTP listener (or a
   * durable target's engine) never leaks its port/resources (FR-MET-2).
   */
  async onConfigurationChange(config: Config): Promise<boolean> {
    try {
      const target = await buildTarget(config, this.messaging, this.targetDefault);
      const previous = this.target;
      this.target = target;
      if (previous !== target) {
        await previous.shutdown().catch(() => undefined);
      }
      return true;
    } catch (e) {
      // eslint-disable-next-line no-console
      console.warn(`failed to rebuild metric target on config change; keeping previous: ${String(e)}`);
      return false;
    }
  }
}
