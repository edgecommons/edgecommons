/**
 * Heartbeat — periodically sample system health and publish it to the metric
 * and/or messaging targets.
 *
 * **One-liner purpose**: A `setInterval` loop that ticks at
 * `heartbeat.intervalSecs` and, for each configured target, either emits the
 * `heartbeat` metric (target `metric`) or publishes a `heartbeat` message
 * (target `messaging`). Stats are collected by {@link HeartbeatMonitor} for the
 * enabled `heartbeat.measures`. Mirrors the Rust `heartbeat.rs`.
 *
 * ## Parity notes
 * - The tick body is wrapped so a transient failure logs and the next tick still
 *   fires — the heartbeat can't be permanently killed by one error.
 * - Stats shape matches Java/Python/Rust: a nested object `{ cpu: {cpu_usage},
 *   memory: {memory_usage}, disk: {disk_total,disk_used,disk_free}, threads:
 *   {threads}, files: {files}, fds: {fds} }`, only including enabled measures.
 *   The metric target flattens it to measure→value; the messaging target sends
 *   it as the message payload.
 * - The live config handle is modeled as a {@link ConfigProvider} getter so each
 *   tick re-reads the latest snapshot (mirrors Rust's `Arc<ArcSwap<Config>>`);
 *   the ticker is rebuilt when `intervalSecs` changes.
 * - Defaults: topic `ggcommons/{ThingName}/{ComponentName}/heartbeat`,
 *   destination `ipc`, interval 5s (minimum 1), message name `heartbeat`,
 *   version `1.0.0`.
 *
 * ## Platform fallbacks (deviations from Rust)
 * Node has no portable disk/thread/fd APIs. cpu/memory use Node built-ins on all
 * platforms; disk uses `fs.statfsSync` (Node 18+) for the cwd filesystem; threads
 * and fds/files read `/proc/self/*` on Linux only. On any platform where a source
 * is unavailable an enabled measure reports `0` (Rust additionally supports
 * Windows via FFI — there is no Windows-native equivalent here, so threads/fds on
 * Windows report `0`). All fallbacks are graceful and never throw.
 */
import * as fs from "fs";
import * as os from "os";

import type { Config, Measures } from "./config/model";
import { resolve } from "./config/template";
import type { MetricService, MeasureValues } from "./metrics/types";
import { MetricBuilder } from "./metrics/metric";
import type { IMessagingService } from "./messaging/types";
import { Qos } from "./messaging/types";
import { MessageBuilder } from "./message";
import { logger } from "./logging";

const MESSAGE_NAME = "heartbeat";
const MESSAGE_VERSION = "1.0.0";
const DEFAULT_INTERVAL_SECS = 5;
const DEFAULT_TOPIC = "ggcommons/{ThingName}/{ComponentName}/heartbeat";
const DEFAULT_DESTINATION = "ipc";

/**
 * A live config handle: a getter returning the current {@link Config} snapshot.
 * Each heartbeat tick calls it to read the latest config (mirrors Rust's
 * `Arc<ArcSwap<Config>>`).
 */
export type ConfigProvider = () => Config;

/** The configured heartbeat interval in seconds (default 5, minimum 1). */
function heartbeatInterval(config: Config): number {
  const secs = config.parsed.heartbeat.intervalSecs ?? DEFAULT_INTERVAL_SECS;
  return Math.max(1, secs);
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

/**
 * Publish `stats` to each configured heartbeat target (best-effort; logs
 * failures). Mirrors the Rust `publish`.
 */
async function publish(
  config: Config,
  metrics: MetricService,
  messaging: IMessagingService | undefined,
  stats: Record<string, unknown>,
): Promise<void> {
  for (const target of config.parsed.heartbeat.targets) {
    const type = target.type.toLowerCase();
    if (type === "metric") {
      try {
        await metrics.emitMetricNow("heartbeat", flatten(stats));
      } catch (e) {
        logger.warn(`heartbeat metric emit failed: ${errMsg(e)}`);
      }
    } else if (type === "messaging") {
      if (!messaging) {
        logger.warn("heartbeat messaging target configured but no messaging service");
        continue;
      }
      const cfg = target.config;
      const topicTemplate = strConfig(cfg, "topic") ?? DEFAULT_TOPIC;
      const topic = resolve(config, topicTemplate);
      const destination = (strConfig(cfg, "destination") ?? DEFAULT_DESTINATION).toLowerCase();

      const message = MessageBuilder.create(MESSAGE_NAME, MESSAGE_VERSION)
        .withPayload(stats)
        .withConfig(config)
        .build();

      try {
        if (destination === "iot_core" || destination === "iotcore") {
          await messaging.publishToIotCore(topic, message, Qos.AtLeastOnce);
        } else if (destination === "ipc" || destination === "local") {
          await messaging.publish(topic, message);
        } else {
          logger.warn(`unrecognized heartbeat messaging destination: ${destination}`);
          continue;
        }
      } catch (e) {
        logger.warn(`heartbeat publish failed: ${errMsg(e)}`);
      }
    } else {
      logger.warn(`unknown heartbeat target type: ${target.type}`);
    }
  }
}

/** Read a string field from a target's `config`, if present. */
function strConfig(cfg: Record<string, unknown> | undefined, key: string): string | undefined {
  const v = cfg?.[key];
  return typeof v === "string" ? v : undefined;
}

function errMsg(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/**
 * Owns the heartbeat background timer. Call {@link stop} to clear it (the RAII
 * analog of dropping the Rust `Heartbeat`).
 */
export class Heartbeat {
  private timer?: NodeJS.Timeout;
  private stopped = false;
  private currentInterval: number;

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
   * Define the `heartbeat` metric and start the periodic publishing task.
   *
   * On start the `heartbeat` metric is defined with all 8 measures (storage
   * resolution `1` when interval `< 60`, else `60`), then a `setInterval` loop at
   * the configured interval (default 5, minimum 1 secs) reads the latest config,
   * rebuilds the timer if `intervalSecs` changed, collects stats for the enabled
   * measures, and publishes to each configured target. Per-tick errors are caught
   * and logged, never killing the loop.
   */
  static start(
    configProvider: ConfigProvider,
    metrics: MetricService,
    messaging?: IMessagingService,
  ): Heartbeat {
    const initial = configProvider();
    const interval = heartbeatInterval(initial);

    // Define the heartbeat metric (all measures, like the Java/Python/Rust libs).
    const storageResolution = interval < 60 ? 1 : 60;
    const metric = MetricBuilder.create("heartbeat")
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
    // Fire the first tick immediately, matching Rust's `tokio::time::interval`
    // which yields its first tick at t=0 (then every `interval` thereafter).
    void hb.tick();

    logger.info(`heartbeat started (interval_secs=${interval})`);
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

  /** One heartbeat cycle. Never throws (errors are caught and logged). */
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

      this.monitor.setMeasures(cfg.parsed.heartbeat.measures);
      const stats = this.monitor.getStats();
      await publish(cfg, this.metrics, this.messaging, stats);
    } catch (e) {
      logger.warn(`heartbeat tick failed: ${errMsg(e)}`);
    }
  }

  /** Stop the heartbeat, clearing the timer (the RAII analog). Idempotent. */
  stop(): void {
    this.stopped = true;
    if (this.timer) {
      clearInterval(this.timer);
      this.timer = undefined;
    }
  }
}
