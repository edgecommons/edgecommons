/**
 * Metrics target — prometheus (pull-based, TypeScript). FR-MET-1/2/3.
 *
 * **One-liner purpose**: maintain an in-process metric registry and serve it as
 * OpenMetrics/Prometheus text over HTTP at `path` (default `/metrics`) on `port`
 * (default 9090, bound `0.0.0.0`), so a Prometheus server (or the kubelet's scrape
 * config) *pulls* metrics on its own cadence. The default metric target on the
 * KUBERNETES platform. Mirrors the canonical Java (io.prometheus) / Python
 * (prometheus-client) / Rust (`prometheus` crate) prometheus targets.
 *
 * ## Inverted lifecycle (FR-MET-2 — the load-bearing difference vs. every push target)
 * The other targets (log / messaging / cloudwatch / cloudwatchcomponent) *push* on
 * `emit`/`flush`. The prometheus target inverts this:
 *  - {@link emit} / {@link emitNow} **update the in-process registry** (latest-value
 *    gauges); they push *nowhere*. There is no batching and no network I/O on emit.
 *  - {@link flush} is a **no-op w.r.t. delivery** — delivery happens when Prometheus
 *    scrapes `GET <path>`, not on flush.
 *  - {@link shutdown} **stops the HTTP listener** (releases the port; no leaked
 *    socket). After shutdown, emits are ignored.
 *
 * This inversion does NOT change the {@link MetricTarget} contract; it only changes
 * what the four methods *do* for this one target.
 *
 * ## Dimension -> label mapping (FR-MET-3 — locked for four-way parity)
 * For each measure in an emitted metric a Gauge is registered/updated:
 *  - **gauge name** = `sanitizeMetricName(lowercase("{namespace}_{measureName}"))`,
 *    where `namespace` is the configured metric namespace (default `edgecommons`);
 *    {@link sanitizeMetricName} replaces every char not in `[a-z0-9_]` with `_` and
 *    prefixes `_` if the result starts with a digit (Prometheus metric-name rules).
 *  - **labels** = the metric's dimensions ({@link Metric.getDimensions} — already
 *    `category`=metric name, `coreName`, `component`, plus any custom dims). Each
 *    label *name* is sanitized by {@link sanitizeLabelName} to
 *    `[a-zA-Z_][a-zA-Z0-9_]*`; the label *value* is used as-is.
 *  - the gauge for that label-set is **set** to the measure's float value on each
 *    emit (latest-value semantics — a scrape reads the current value).
 *
 * `prom-client` is an optional dependency, lazily imported by the metrics service so
 * merely importing the library never pulls it in; its absence is a
 * {@link EdgeCommonsError.metrics} at {@link PrometheusTarget.create} (mirrors the cloudwatch
 * targets). The exposition uses the client's own writer, which sets a valid
 * `Content-Type` (`text/plain; version=0.0.4; charset=utf-8`) — Prometheus 3.x
 * rejects a blank content type.
 */
import * as http from "http";
import type { AddressInfo } from "net";

import type { MetricTarget, MeasureValues } from "../types";
import type { Metric } from "../metric";
import { EdgeCommonsError } from "../../errors";
import { logger } from "../../logging";

/** Minimal structural view of the prom-client `Gauge` bits we use (avoids a hard type dep). */
interface GaugeLike {
  set(labels: Record<string, string>, value: number): void;
}

/** Minimal structural view of the prom-client `Registry` bits we use. */
interface RegistryLike {
  /** The valid OpenMetrics/Prometheus content type for the exposition response. */
  contentType: string;
  /** Render the current registry as Prometheus text (Promise in prom-client v13+). */
  metrics(): Promise<string> | string;
  /** Remove a single metric by name (used to widen a gauge's label set). */
  removeSingleMetric(name: string): void;
}

/** Minimal structural view of the parts of `prom-client` we depend on. */
interface PromModule {
  Registry: new () => RegistryLike;
  Gauge: new (cfg: {
    name: string;
    help: string;
    labelNames: string[];
    registers: RegistryLike[];
  }) => GaugeLike;
}

/**
 * Sanitize a Prometheus **metric** name (FR-MET-3): lower-case, then replace every char not in
 * `[a-z0-9_]` with `_`, then prefix `_` if the result starts with a digit. Identical across all four
 * languages.
 */
export function sanitizeMetricName(raw: string): string {
  let s = raw.toLowerCase().replace(/[^a-z0-9_]/g, "_");
  if (/^[0-9]/.test(s)) s = `_${s}`;
  return s;
}

/**
 * Sanitize a Prometheus **label** name (FR-MET-3): replace every char not in `[a-zA-Z0-9_]` with `_`
 * (case is preserved, unlike the metric name), then prefix `_` if it starts with a digit. Identical
 * across all four languages.
 */
export function sanitizeLabelName(raw: string): string {
  let s = raw.replace(/[^a-zA-Z0-9_]/g, "_");
  if (/^[0-9]/.test(s)) s = `_${s}`;
  return s;
}

/** Whether two already-sorted label-name lists are identical (same set + order). */
function sameLabelNames(a: string[], b: string[]): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) {
    if (a[i] !== b[i]) return false;
  }
  return true;
}

/**
 * A pull-based prometheus metric target: an in-process gauge registry served as OpenMetrics text over
 * HTTP. See the module doc for the inverted lifecycle and the dimension->label mapping.
 */
export class PrometheusTarget implements MetricTarget {
  private readonly mod: PromModule;
  private readonly registry: RegistryLike;
  private readonly namespace: string;
  private readonly httpPath: string;
  private server?: http.Server;
  private closed = false;
  /** gauge-name -> { gauge, declared (sorted) label names }. */
  private readonly gauges = new Map<string, { gauge: GaugeLike; labelNames: string[] }>();

  private constructor(mod: PromModule, registry: RegistryLike, namespace: string, httpPath: string) {
    this.mod = mod;
    this.registry = registry;
    this.namespace = namespace;
    this.httpPath = httpPath;
  }

  /**
   * Build the target: lazily import `prom-client` (a {@link EdgeCommonsError.metrics} if absent), create a
   * fresh registry (NOT the process-global one, so no default process metrics leak in), and start the
   * exposition HTTP server bound `0.0.0.0:<port>` at `path`. Resolves once listening.
   *
   * @param namespace the configured metric namespace (default `edgecommons`) — the gauge-name prefix.
   * @param port TCP port to bind (default 9090; `0` for an ephemeral port in tests).
   * @param path the HTTP path the exposition is served at (default `/metrics`).
   */
  static async create(namespace: string, port: number, path: string): Promise<PrometheusTarget> {
    let mod: PromModule;
    try {
      mod = (await import("prom-client")) as unknown as PromModule;
    } catch {
      throw EdgeCommonsError.metrics("metric target 'prometheus' requires the optional 'prom-client' dependency");
    }
    const registry = new mod.Registry();
    const target = new PrometheusTarget(mod, registry, namespace, path);
    await target.startServer(port, path);
    return target;
  }

  /** Bind the exposition server on `0.0.0.0:<port>` and resolve once listening (reject on bind error). */
  private startServer(port: number, path: string): Promise<void> {
    const server = http.createServer((req, res) => {
      void this.handle(req, res, path);
    });
    // Never keep the event loop alive solely for the metrics server (parity with the health server).
    server.unref();
    this.server = server;
    return new Promise<void>((resolve, reject) => {
      const onError = (err: Error): void => reject(err);
      server.once("error", onError);
      server.listen(port, "0.0.0.0", () => {
        server.removeListener("error", onError);
        logger.info(`prometheus metrics endpoint listening on 0.0.0.0:${this.port()} (path=${path})`);
        resolve();
      });
    });
  }

  /** Route one request: `GET <path>` -> exposition; other paths -> 404; other methods -> 405. */
  private async handle(req: http.IncomingMessage, res: http.ServerResponse, path: string): Promise<void> {
    if (req.method !== "GET") {
      res.writeHead(405, { "Content-Type": "text/plain; charset=utf-8" });
      res.end("method not allowed");
      return;
    }
    const reqPath = (req.url ?? "").split("?")[0];
    if (reqPath !== path) {
      res.writeHead(404, { "Content-Type": "text/plain; charset=utf-8" });
      res.end("not found");
      return;
    }
    try {
      const body = await this.registry.metrics();
      // The client's content type is non-blank/valid (Prometheus 3.x rejects a blank type).
      res.writeHead(200, { "Content-Type": this.registry.contentType });
      res.end(body);
    } catch (e) {
      res.writeHead(500, { "Content-Type": "text/plain; charset=utf-8" });
      res.end(`error rendering metrics: ${String(e)}`);
    }
  }

  /**
   * Update the in-process gauges for one emission (the only place this target "delivers" — into its own
   * registry, NOT over the network). One gauge per measure; the metric's dimensions become labels.
   */
  private update(metric: Metric, values: MeasureValues): void {
    if (this.closed) return;
    const labels: Record<string, string> = {};
    for (const [key, value] of metric.getDimensions()) {
      labels[sanitizeLabelName(key)] = value;
    }
    const labelNames = Object.keys(labels).sort();
    for (const [measureName, value] of Object.entries(values)) {
      const name = sanitizeMetricName(`${this.namespace}_${measureName}`);
      const gauge = this.gaugeFor(name, labelNames);
      if (gauge === undefined) continue; // mismatched label set — warned + skipped
      gauge.set(labels, value);
    }
  }

  /**
   * Fetch (or lazily register) the gauge named `name`. prom-client fixes a gauge's label names at
   * registration. To match the canonical Java/Python/Rust targets, a later emit whose label-name set
   * DIFFERS from the first registration is **dropped with a warning** (the label set is fixed at first
   * registration). We deliberately do NOT widen/re-register, which would reset the series and silently
   * accept missing labels — and would diverge from the other three languages (FR-MET-3 parity).
   */
  private gaugeFor(name: string, labelNames: string[]): GaugeLike | undefined {
    const existing = this.gauges.get(name);
    if (existing) {
      if (!sameLabelNames(existing.labelNames, labelNames)) {
        logger.warn(
          `prometheus gauge '${name}' already registered with labels [${existing.labelNames.join(",")}] ` +
            `but this emit has labels [${labelNames.join(",")}]; skipping`,
        );
        return undefined;
      }
      return existing.gauge;
    }
    const gauge = new this.mod.Gauge({
      name,
      help: `edgecommons metric ${name}`,
      labelNames,
      registers: [this.registry],
    });
    this.gauges.set(name, { gauge, labelNames });
    return gauge;
  }

  /** The actually-bound port (useful when started on the ephemeral port `0` in tests). */
  port(): number {
    const addr = this.server?.address();
    return addr && typeof addr === "object" ? (addr as AddressInfo).port : 0;
  }

  /** FR-MET-2: update the registry (push nowhere — the scrape pulls). */
  async emit(metric: Metric, values: MeasureValues): Promise<void> {
    this.update(metric, values);
  }

  /** FR-MET-2: emitNow behaves identically to emit (no batching; the scrape pulls). */
  async emitNow(metric: Metric, values: MeasureValues): Promise<void> {
    this.update(metric, values);
  }

  /** FR-MET-2: no-op w.r.t. delivery — there is nothing to flush; Prometheus pulls on scrape. */
  async flush(): Promise<void> {
    // Intentionally empty: the prometheus target never pushes.
  }

  /** FR-MET-2: stop the HTTP listener so the port/socket is released (no leak). Idempotent. */
  async shutdown(): Promise<void> {
    if (this.closed) return;
    this.closed = true;
    if (this.server) {
      const server = this.server;
      await new Promise<void>((resolve) => server.close(() => resolve()));
    }
  }
}
