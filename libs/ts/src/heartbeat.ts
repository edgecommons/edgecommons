/**
 * Heartbeat — the component's UNS `state` keepalive + system measures
 * (UNS-CANONICAL-DESIGN §4.3, D-U14/D-U20).
 *
 * **One-liner purpose**: a timer loop that ticks at `heartbeat.intervalSecs` (default 5 s,
 * default ON) and, each tick:
 * 1. publishes a UNS **state keepalive** to `ecv1[/{site}]/{device}/{component}/main/state` —
 *    header name `state`, body `{"status":"RUNNING","uptimeSecs":n}` — through the privileged
 *    reserved-publish seam (the `state` class is library-owned); `heartbeat.destination`
 *    (`local`|`northbound`) selects the keepalive's transport only;
 * 2. emits the enabled system measures (cpu/memory/disk/…) as a metric named **`sys`** through
 *    the normal metric subsystem (D6/D-U20 — the measures keep the metric subsystem's full
 *    sink routing).
 *
 * On graceful shutdown ({@link Heartbeat.stop}) a best-effort `{"status":"STOPPED"}` state is
 * published at most once. The legacy `heartbeat.targets[]` topic-override knobs are removed —
 * hard cut (M11).
 *
 * ## Parity notes
 * - The tick body is wrapped so a transient failure logs and the next tick still fires; the
 *   state and metric halves are individually best-effort (a failure in one must not suppress
 *   the other).
 * - Stats shape matches Java/Python/Rust: a nested object `{ cpu: {cpu_usage}, memory:
 *   {memory_usage}, disk: {disk_total,disk_used,disk_free}, threads: {threads}, files:
 *   {files}, fds: {fds} }`, only including enabled measures; the `sys` metric flattens it to
 *   measure→value.
 * - The live config handle is modeled as a {@link ConfigProvider} getter so each tick re-reads
 *   the latest snapshot; the ticker is rebuilt when `intervalSecs` changes and no-ops while
 *   `heartbeat.enabled` is `false`.
 *
 * ## Platform fallbacks (deviations from Rust)
 * Node has no portable disk/thread/fd APIs. cpu/memory use Node built-ins on all
 * platforms; disk uses `fs.statfsSync` (Node 18+) for the cwd filesystem; threads
 * and fds/files read `/proc/self/*` on Linux only. On any platform where a source
 * is unavailable an enabled measure reports `0`. All fallbacks are graceful and never throw.
 */
import * as fs from "fs";
import * as os from "os";

import type { Config, Measures } from "./config/model";
import type { MetricService, MeasureValues } from "./metrics/types";
import { MetricBuilder } from "./metrics/metric";
import type { IMessagingService } from "./messaging/types";
import { publishReservedVia } from "./messaging/service";
import { MessageBuilder } from "./message";
import { Uns, UnsClass } from "./uns";
import { logger } from "./logging";
import type { InstanceConnectivity, InstanceConnectivityProvider } from "./instance_connectivity";

/** The state keepalive's envelope header name (§4.3). */
const STATE_MESSAGE_NAME = "state";
const STATE_MESSAGE_VERSION = "1.0";
/** The metric the heartbeat measures are emitted as (§4.3, D-U20/D6). */
const SYS_METRIC_NAME = "sys";

/**
 * A live config handle: a getter returning the current {@link Config} snapshot.
 * Each heartbeat tick calls it to read the latest config (mirrors Rust's
 * `Arc<ArcSwap<Config>>`).
 */
export type ConfigProvider = () => Config;

/** The configured heartbeat interval in seconds (default 5, minimum 1 — enforced at parse). */
function heartbeatInterval(config: Config): number {
  return config.parsed.heartbeat.intervalSecs;
}

/** Flatten the nested stats object into a flat `measure -> value` map. */
function flatten(stats: Record<string, unknown>): MeasureValues {
  const out: MeasureValues = {};
  for (const category of Object.values(stats)) {
    if (category && typeof category === "object" && !Array.isArray(category)) {
      for (const [name, value] of Object.entries(category as Record<string, unknown>)) {
        if (typeof value === "number") {
          out[name] = value;
        }
      }
    }
  }
  return out;
}

/**
 * Collects system health statistics for the enabled measures.
 *
 * CPU usage is measured as a delta between consecutive {@link getStats} calls, so
 * the value is meaningful only when the calls are spaced by a real interval (the
 * heartbeat period). The first sample has no baseline and reports `0.0`, matching
 * psutil's first `cpu_percent()` and the Rust monitor. The reported value follows
 * the convention where `100%` is one fully-used core (it can exceed 100% for a
 * multi-threaded process).
 */
export class HeartbeatMonitor {
  private measures: Measures;
  /** Undefined until the first sample establishes a CPU baseline. */
  private lastCpu?: NodeJS.CpuUsage;
  private lastHrtime?: bigint;

  constructor(measures: Measures) {
    this.measures = measures;
  }

  /** Update which measures are collected (used when config is hot-reloaded). */
  setMeasures(measures: Measures): void {
    this.measures = measures;
  }

  /** Collect the enabled measures as a nested plain object. */
  getStats(): Record<string, unknown> {
    // CPU delta over the elapsed interval, as a percent of one core. The first
    // sample has no baseline and reports 0.0.
    const nowCpu = process.cpuUsage();
    const nowHr = process.hrtime.bigint();
    let cpuUsage = 0;
    if (this.lastCpu !== undefined && this.lastHrtime !== undefined) {
      const elapsedMicros = Number(nowHr - this.lastHrtime) / 1000; // ns -> µs
      if (elapsedMicros > 0) {
        const usedMicros =
          nowCpu.user - this.lastCpu.user + (nowCpu.system - this.lastCpu.system);
        cpuUsage = (usedMicros / elapsedMicros) * 100;
      }
    }
    this.lastCpu = nowCpu;
    this.lastHrtime = nowHr;

    const data: Record<string, unknown> = {};
    if (this.measures.cpu) {
      data.cpu = { cpu_usage: cpuUsage };
    }
    if (this.measures.memory) {
      data.memory = { memory_usage: process.memoryUsage().rss / 1_000_000 };
    }
    if (this.measures.disk) {
      const [total, used, free] = diskUsageGb();
      data.disk = { disk_total: total, disk_used: used, disk_free: free };
    }
    if (this.measures.threads) {
      data.threads = { threads: threadCount() ?? 0 };
    }
    if (this.measures.files) {
      data.files = { files: openFileCount() ?? 0 };
    }
    if (this.measures.fds) {
      data.fds = { fds: fdCount() ?? 0 };
    }
    return data;
  }
}

/**
 * Disk total/used/free in gigabytes for the filesystem holding the current dir.
 * Best-effort: uses `fs.statfsSync` (Node 18+); returns `0/0/0` if unavailable.
 */
function diskUsageGb(): [number, number, number] {
  try {
    const statfs = (fs as unknown as { statfsSync?: (p: string) => StatFs }).statfsSync;
    if (typeof statfs !== "function") {
      return [0, 0, 0];
    }
    const s = statfs(process.cwd());
    const total = s.blocks * s.bsize;
    const free = s.bavail * s.bsize;
    const used = total - free;
    return [total / 1e9, used / 1e9, free / 1e9];
  } catch {
    return [0, 0, 0];
  }
}

/** Minimal shape of the `fs.StatsFs` object returned by `fs.statfsSync`. */
interface StatFs {
  bsize: number;
  blocks: number;
  bavail: number;
}

// ----- Platform-specific process counters (best-effort) -----

/** Thread count: Linux `/proc/self/task` entry count; else `undefined`. */
function threadCount(): number | undefined {
  if (os.platform() !== "linux") {
    return undefined;
  }
  try {
    return fs.readdirSync("/proc/self/task").length;
  } catch {
    return undefined;
  }
}

/** Open fd count: Linux `/proc/self/fd` entry count; else `undefined`. */
function fdCount(): number | undefined {
  if (os.platform() !== "linux") {
    return undefined;
  }
  try {
    return fs.readdirSync("/proc/self/fd").length;
  } catch {
    return undefined;
  }
}

/**
 * Open file count: like the Rust/psutil convention, report the total fd count on
 * Linux (`/proc/self/fd`); else `undefined`.
 */
function openFileCount(): number | undefined {
  return fdCount();
}

function errMsg(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/**
 * Owns the heartbeat background timer. Call {@link stop} to clear it and publish the
 * best-effort STOPPED state (the RAII analog of Java `Heartbeat.close()`).
 */
export class Heartbeat {
  private timer?: NodeJS.Timeout;
  private stopped = false;
  private currentInterval: number;
  /** Monotonic start reference for the keepalive's `uptimeSecs`. */
  private readonly startHr = process.hrtime.bigint();
  /** Ensures the best-effort STOPPED state is published at most once. */
  private stoppedPublished = false;
  /**
   * An optional component-supplied source of per-instance connectivity, sampled each keepalive
   * tick into the state body's `instances` array (see {@link InstanceConnectivityProvider}).
   */
  private connectivityProvider?: InstanceConnectivityProvider;

  private constructor(
    private readonly configProvider: ConfigProvider,
    private readonly metrics: MetricService,
    private readonly messaging: IMessagingService | undefined,
    private readonly monitor: HeartbeatMonitor,
    interval: number,
  ) {
    this.currentInterval = interval;
  }

  /**
   * Registers (or clears with `undefined`) the per-instance connectivity provider whose result is
   * emitted in each RUNNING `state` keepalive's `instances` array — the overridable surface a
   * multi-connection component uses to report connectivity at the instance level without a separate
   * UNS instance per connection. Wired from `EdgeCommons.setInstanceConnectivityProvider`.
   */
  setInstanceConnectivityProvider(provider: InstanceConnectivityProvider | undefined): void {
    this.connectivityProvider = provider;
  }

  /**
   * Samples the registered per-instance connectivity provider, once.
   *
   * This is the single sampling seam, and it deliberately serves **both** surfaces: the `state`
   * keepalive pushes it in `instances`, and the built-in `status` verb (`CommandInbox.STATUS`)
   * returns it when pulled. One component-supplied provider; two surfaces; no second copy of the
   * data to drift out of step.
   *
   * Best-effort by contract: no provider, a nullish result, or a throwing provider all yield an
   * empty array. A component's provider bug must never suppress the keepalive or fail the command —
   * it can only cost the `instances` section for that sample.
   *
   * @returns the live per-instance connectivity; never nullish, possibly empty
   */
  sampleInstanceConnectivity(): InstanceConnectivity[] {
    const provider = this.connectivityProvider;
    if (!provider) {
      return [];
    }
    try {
      const conns = provider();
      if (!conns || conns.length === 0) {
        return [];
      }
      return conns.filter((c): c is InstanceConnectivity => c != null);
    } catch (e) {
      logger.warn(`instance connectivity provider failed; omitting instances[] this sample: ${errMsg(e)}`);
      return [];
    }
  }

  /**
   * Define the `sys` metric (the heartbeat measures) and start the periodic task.
   *
   * On start the `sys` metric is defined with all 8 measures (storage resolution `1` when
   * interval `< 60`, else `60`), then a `setInterval` loop at the configured interval reads
   * the latest config, rebuilds the timer if `intervalSecs` changed, and — while
   * `heartbeat.enabled` (the default) — publishes the state keepalive and the `sys` metric.
   * Per-tick errors are caught and logged, never killing the loop.
   */
  static start(
    configProvider: ConfigProvider,
    metrics: MetricService,
    messaging?: IMessagingService,
  ): Heartbeat {
    const initial = configProvider();
    const interval = heartbeatInterval(initial);

    // Define the sys metric (all measures, like the Java/Python/Rust libs).
    const storageResolution = interval < 60 ? 1 : 60;
    const metric = MetricBuilder.create(SYS_METRIC_NAME)
      .withConfig(initial)
      .addMeasure("disk_total", "Gigabytes", storageResolution)
      .addMeasure("disk_used", "Gigabytes", storageResolution)
      .addMeasure("disk_free", "Gigabytes", storageResolution)
      .addMeasure("cpu_usage", "Percent", storageResolution)
      .addMeasure("memory_usage", "Megabytes", storageResolution)
      .addMeasure("threads", "Count", storageResolution)
      .addMeasure("files", "Count", storageResolution)
      .addMeasure("fds", "Count", storageResolution)
      .build();
    metrics.defineMetric(metric);

    const monitor = new HeartbeatMonitor(initial.parsed.heartbeat.measures);
    const hb = new Heartbeat(configProvider, metrics, messaging, monitor, interval);
    hb.arm();
    // Fire the first tick immediately, matching the Java scheduleAtFixedRate initial delay 0
    // and Rust's `tokio::time::interval` first tick at t=0.
    void hb.tick();

    if (initial.parsed.heartbeat.enabled) {
      logger.info(
        `heartbeat started (interval_secs=${interval}, state keepalive -> ${initial.parsed.heartbeat.destination})`,
      );
    } else {
      logger.info("heartbeat disabled by configuration (heartbeat.enabled=false)");
    }
    return hb;
  }

  /** (Re)create the interval timer at the current interval. */
  private arm(): void {
    this.timer = setInterval(() => {
      void this.tick();
    }, this.currentInterval * 1000);
    // Do not keep the event loop alive solely for the heartbeat.
    if (typeof this.timer.unref === "function") {
      this.timer.unref();
    }
  }

  /**
   * The component's monotonic uptime in whole seconds — the same value the RUNNING `state`
   * keepalive carries as `uptimeSecs`. Consumed by the command inbox's `ping` built-in verb
   * (DESIGN-uns §9.5) so ping replies and keepalives agree on one uptime source.
   */
  getUptimeSecs(): number {
    return Number((process.hrtime.bigint() - this.startHr) / 1_000_000_000n);
  }

  /**
   * Publishes one `state` envelope to the component's UNS state topic through the privileged
   * seam (§4.3).
   *
   * @param status        `"RUNNING"` or `"STOPPED"`
   * @param includeUptime whether the body carries `uptimeSecs` (the RUNNING keepalive)
   */
  private async publishState(cfg: Config, status: string, includeUptime: boolean): Promise<void> {
    if (!this.messaging) {
      return;
    }
    const topic = new Uns(cfg.componentIdentity, cfg.topicIncludeRoot).topic(UnsClass.State);
    const body: Record<string, unknown> = { status };
    if (includeUptime) {
      body.uptimeSecs = this.getUptimeSecs();
    }
    // Per-instance connectivity — the state body's `instances` (only on the RUNNING keepalive; a
    // STOPPED state carries no live instances). Sampled through the one seam the `status` verb also
    // pulls, so the pushed and pulled answers cannot diverge.
    if (includeUptime) {
      const instances = this.sampleInstanceConnectivity().map((c) => c.toJson());
      if (instances.length > 0) {
        body.instances = instances;
      }
    }
    const stateMessage = MessageBuilder.create(STATE_MESSAGE_NAME, STATE_MESSAGE_VERSION)
      .withPayload(body)
      .withConfig(cfg)
      .build();
    const destination = cfg.parsed.heartbeat.destination.toLowerCase();
    const dest = destination === "northbound" ? "northbound" : "local";
    await publishReservedVia(this.messaging, topic, stateMessage, dest);
  }

  /**
   * Re-emits the RUNNING `state` keepalive immediately, out of band from the periodic
   * schedule — the `republish-state` broadcast re-announce (DESIGN-uns §9.3/§9.4, the
   * late-join lever, `RepublishListener`): same payload as a tick's keepalive
   * (`{"status":"RUNNING","uptimeSecs":n}`), same privileged reserved-publish seam, same
   * `heartbeat.destination` routing. Respects `heartbeat.enabled`: a component whose operator
   * disabled the state keepalive does not re-announce state (the broadcast cannot re-enable an
   * opted-out state surface). Best-effort — failures are logged and swallowed; the periodic
   * schedule is unaffected.
   */
  async publishStateNow(): Promise<void> {
    const cfg = this.configProvider();
    if (!cfg.parsed.heartbeat.enabled) {
      logger.debug(
        "republish-state re-announce skipped: the heartbeat state keepalive is disabled (heartbeat.enabled=false)",
      );
      return;
    }
    try {
      await this.publishState(cfg, "RUNNING", true);
    } catch (e) {
      logger.warn(`out-of-band state re-announce failed: ${errMsg(e)}`);
    }
  }

  /**
   * One heartbeat cycle (§4.3): the `state` keepalive plus the measures as the `sys` metric.
   * Each half is best-effort — a failure in one must not suppress the other. Never throws.
   */
  private async tick(): Promise<void> {
    if (this.stopped) {
      return;
    }
    try {
      const cfg = this.configProvider();

      // React to an interval change by rebuilding the ticker.
      const newInterval = heartbeatInterval(cfg);
      if (newInterval !== this.currentInterval) {
        this.currentInterval = newInterval;
        if (this.timer) {
          clearInterval(this.timer);
        }
        this.arm();
      }

      if (!cfg.parsed.heartbeat.enabled) {
        return;
      }

      try {
        await this.publishState(cfg, "RUNNING", true);
      } catch (e) {
        logger.warn(`heartbeat state keepalive failed: ${errMsg(e)}`);
      }
      try {
        this.monitor.setMeasures(cfg.parsed.heartbeat.measures);
        const stats = this.monitor.getStats();
        await this.metrics.emitMetricNow(SYS_METRIC_NAME, flatten(stats));
      } catch (e) {
        logger.warn(`heartbeat '${SYS_METRIC_NAME}' metric emit failed: ${errMsg(e)}`);
      }
    } catch (e) {
      logger.warn(`heartbeat tick failed: ${errMsg(e)}`);
    }
  }

  /**
   * Stop the heartbeat: clear the timer and publish the best-effort `{"status":"STOPPED"}`
   * state (§4.3/D-U14 — at most once; failures are swallowed, the shutdown must proceed).
   * Idempotent. Await it BEFORE disconnecting messaging so the STOPPED state can leave.
   */
  async stop(): Promise<void> {
    const wasRunning = this.timer !== undefined && !this.stopped;
    this.stopped = true;
    if (this.timer) {
      clearInterval(this.timer);
      this.timer = undefined;
    }
    if (wasRunning && !this.stoppedPublished) {
      this.stoppedPublished = true;
      try {
        const cfg = this.configProvider();
        if (cfg.parsed.heartbeat.enabled) {
          await this.publishState(cfg, "STOPPED", false);
        }
      } catch (e) {
        logger.debug(`best-effort STOPPED state publish failed: ${errMsg(e)}`);
      }
    }
  }
}
