/**
 * CloudWatch metric-target verification (AWS-direct, no nucleus).
 *
 * Builds a `MetricEmitter` with `metricEmission.target = "cloudwatch"` and emits a
 * metric carrying a unique `token` dimension, exercising the real
 * `@aws-sdk/client-cloudwatch` PutMetricData path. A separate step then queries
 * CloudWatch for the namespace/token to confirm the datum landed. This validates
 * the `cloudwatch` metric target (and, indirectly, heartbeat→cloudwatch, which uses
 * the same target) against the real service.
 *
 * Run with working AWS creds + a region:
 *   AWS_REGION=us-east-1 CW_NS=ggcommons-ts-verify CW_TOKEN=<unique> node dist/cw_verify.js
 */
import { Config } from "./config/model";
import { MetricBuilder } from "./metrics/metric";
import { MetricEmitter } from "./metrics/service";

const NS = process.env.CW_NS ?? "ggcommons-ts-verify";
const TOKEN = process.env.CW_TOKEN ?? String(Date.now());

async function main(): Promise<void> {
  const raw = {
    metricEmission: { target: "cloudwatch", namespace: NS, targetConfig: { intervalSecs: 1 } },
  };
  const cfg = Config.fromValue("com.ggcommons.CwVerify", "lab-5950x", raw);

  const out: Record<string, unknown> = { namespace: NS, token: TOKEN };
  try {
    const emitter = await MetricEmitter.create(cfg);
    emitter.defineMetric(
      MetricBuilder.create("verify")
        .withNamespace(NS)
        .withThingName("lab-5950x")
        .addMeasure("count", "Count", 60)
        .addDimension("token", TOKEN)
        .build(),
    );
    await emitter.emitMetricNow("verify", { count: 1 });
    await emitter.flushMetrics();
    await emitter.shutdown();
    out.ok = true;
  } catch (e) {
    out.ok = false;
    out.error = String(e);
  }
  process.stdout.write(JSON.stringify(out) + "\n");
  process.exit(out.ok ? 0 : 1);
}

void main();
