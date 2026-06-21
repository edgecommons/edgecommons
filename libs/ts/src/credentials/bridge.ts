/**
 * Periodically surfaces the credential subsystem's non-sensitive {@link CredentialStats} through the
 * component metric service (CloudWatch / messaging / log), so they land in the same configured target
 * as heartbeat. Mirrors the Rust `CredentialMetricsBridge` and the TS `StreamMetricsBridge`.
 * **Never emits secret values.**
 */
import type { Config } from "../config/model";
import { logger } from "../logging";
import { MetricBuilder } from "../metrics/metric";
import type { MetricService } from "../metrics/types";

import type { CredentialService } from "./service";

const DEFAULT_INTERVAL_SECS = 30;
const METRIC = "credentials";
const MEASURES: ReadonlyArray<readonly [string, string]> = [
  ["secretCount", "Count"],
  ["lastSyncAgeMs", "Milliseconds"],
  ["syncFailures", "Count"],
  ["rotations", "Count"],
];

export class CredentialMetricsBridge {
  private timer?: ReturnType<typeof setInterval>;

  constructor(
    config: Config,
    private readonly metrics: MetricService,
    private readonly credentials: CredentialService,
    intervalSecs: number = DEFAULT_INTERVAL_SECS,
  ) {
    const resolution = intervalSecs < 60 ? 1 : 60;
    let builder = MetricBuilder.create(METRIC).withConfig(config);
    for (const [measure, unit] of MEASURES) builder = builder.addMeasure(measure, unit, resolution);
    metrics.defineMetric(builder.build());
    this.timer = setInterval(() => {
      void this.tick();
    }, intervalSecs * 1000);
    this.timer.unref?.();
    logger.info(`Credential metrics bridge started at ${intervalSecs}s interval`);
  }

  private async tick(): Promise<void> {
    try {
      const s = this.credentials.stats();
      await this.metrics.emitMetric(METRIC, {
        secretCount: s.secretCount,
        lastSyncAgeMs: s.lastSyncAgeMs ?? 0,
        syncFailures: s.syncFailures,
        rotations: s.rotations,
      });
    } catch (e) {
      logger.debug(`failed to emit credential stats: ${String(e)}`);
    }
  }

  close(): void {
    if (this.timer) {
      clearInterval(this.timer);
      this.timer = undefined;
    }
  }
}
