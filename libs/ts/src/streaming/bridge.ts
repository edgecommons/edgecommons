/**
 * Periodically emits each telemetry stream's stats through the component metric service, so
 * streaming metrics land in the same configured target (CloudWatch / messaging / log) as heartbeat.
 * Mirrors the Rust/Java/Python `StreamMetricsBridge`; one metric per stream, `stream:<name>`.
 */
import type { Config } from "../config/model";
import { logger } from "../logging";
import { MetricBuilder } from "../metrics/metric";
import type { MetricService } from "../metrics/types";

import type { StreamService } from "./service";

const DEFAULT_INTERVAL_SECS = 30;
const MEASURES: ReadonlyArray<readonly [string, string]> = [
  ["backlog", "Count"],
  ["droppedTotal", "Count"],
  ["exportedTotal", "Count"],
  ["retriesTotal", "Count"],
  ["failedTotal", "Count"],
  ["diskBytes", "Bytes"],
  ["oldestUnackedAgeMs", "Milliseconds"],
];

export class StreamMetricsBridge {
  private timer?: ReturnType<typeof setInterval>;

  constructor(
    config: Config,
    private readonly metrics: MetricService,
    private readonly streams: StreamService,
    private readonly names: string[],
    intervalSecs: number = DEFAULT_INTERVAL_SECS,
  ) {
    const resolution = intervalSecs < 60 ? 1 : 60;
    for (const name of names) {
      let builder = MetricBuilder.create(`stream:${name}`).withConfig(config);
      for (const [measure, unit] of MEASURES) builder = builder.addMeasure(measure, unit, resolution);
      metrics.defineMetric(builder.build());
    }
    this.timer = setInterval(() => {
      void this.tick();
    }, intervalSecs * 1000);
    this.timer.unref?.();
    logger.info(`Stream metrics bridge started for ${names.length} stream(s) at ${intervalSecs}s interval`);
  }

  private async tick(): Promise<void> {
    for (const name of this.names) {
      try {
        const s = this.streams.stats(name);
        await this.metrics.emitMetric(`stream:${name}`, {
          backlog: s.backlog,
          droppedTotal: s.droppedTotal,
          exportedTotal: s.exportedTotal,
          retriesTotal: s.retriesTotal,
          failedTotal: s.failedTotal,
          diskBytes: s.diskBytes,
          oldestUnackedAgeMs: s.oldestUnackedAgeMs,
        });
      } catch (e) {
        logger.debug(`failed to emit stats for stream ${name}: ${String(e)}`);
      }
    }
  }

  close(): void {
    if (this.timer) {
      clearInterval(this.timer);
      this.timer = undefined;
    }
  }
}
