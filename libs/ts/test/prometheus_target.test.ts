/**
 * Unit tests for the pull-based prometheus metric target (`src/metrics/target/prometheus.ts`) and its
 * selection via `metricEmission.target=prometheus` (FR-MET-1/2/3). Mirrors the canonical Java/Python/
 * Rust prometheus-target tests. Real loopback `GET <path>` exercises the exposition where practical;
 * ephemeral ports (`0`) keep the suite hermetic.
 */
import { describe, it, expect, afterEach } from "vitest";
import * as http from "http";
import * as net from "net";
import type { AddressInfo } from "net";

import {
  PrometheusTarget,
  sanitizeMetricName,
  sanitizeLabelName,
} from "../src/metrics/target/prometheus";
import { MetricBuilder } from "../src/metrics/metric";
import { Config } from "../src/config/model";
import { MetricEmitter } from "../src/metrics/service";

/** Targets created during a test, shut down in afterEach so no listener leaks. */
const opened: Array<{ shutdown(): Promise<void> }> = [];
afterEach(async () => {
  for (const t of opened.splice(0)) {
    await t.shutdown().catch(() => undefined);
  }
});

function track<T extends { shutdown(): Promise<void> }>(t: T): T {
  opened.push(t);
  return t;
}

function metric(): ReturnType<MetricBuilder["build"]> {
  return MetricBuilder.create("requests")
    .withThingName("thing-1")
    .withComponentName("com.example.C")
    .withNamespace("ns")
    .addMeasure("count", "Count", 60)
    .build();
}

/** A loopback HTTP GET returning status + body + content-type. */
function httpGet(port: number, path: string): Promise<{ status: number; body: string; contentType?: string }> {
  return new Promise((resolve, reject) => {
    const req = http.get({ host: "127.0.0.1", port, path }, (res) => {
      let body = "";
      res.setEncoding("utf8");
      res.on("data", (c) => (body += c));
      res.on("end", () =>
        resolve({ status: res.statusCode ?? 0, body, contentType: res.headers["content-type"] }),
      );
    });
    req.on("error", reject);
  });
}

/** Find a free TCP port (bind :0, read it, release) so an emitter-selected target can use it. */
function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const srv = net.createServer();
    srv.once("error", reject);
    srv.listen(0, "127.0.0.1", () => {
      const p = (srv.address() as AddressInfo).port;
      srv.close(() => resolve(p));
    });
  });
}

describe("sanitizeMetricName (FR-MET-3)", () => {
  it("lowercases and replaces invalid chars with '_'", () => {
    expect(sanitizeMetricName("edgecommons_RequestCount")).toBe("edgecommons_requestcount");
    expect(sanitizeMetricName("ns.foo-bar/baz")).toBe("ns_foo_bar_baz");
    expect(sanitizeMetricName("a b%c")).toBe("a_b_c");
  });

  it("prefixes '_' when starting with a digit", () => {
    expect(sanitizeMetricName("9lives")).toBe("_9lives");
    expect(sanitizeMetricName("0_count")).toBe("_0_count");
  });
});

describe("sanitizeLabelName (FR-MET-3)", () => {
  it("preserves case but replaces invalid chars", () => {
    expect(sanitizeLabelName("coreName")).toBe("coreName");
    expect(sanitizeLabelName("my-dim.x")).toBe("my_dim_x");
    expect(sanitizeLabelName("a+b#c")).toBe("a_b_c");
  });

  it("prefixes '_' when starting with a digit", () => {
    expect(sanitizeLabelName("1region")).toBe("_1region");
  });
});

describe("PrometheusTarget", () => {
  it("emit updates the registry and /metrics serves OpenMetrics text with the right gauge + labels", async () => {
    const t = track(await PrometheusTarget.create("ns", 0, "/metrics"));
    await t.emit(metric(), { count: 5 });

    const { status, body, contentType } = await httpGet(t.port(), "/metrics");
    expect(status).toBe(200);
    // Valid (non-blank) Content-Type — Prometheus 3.x rejects a blank type.
    expect(contentType).toContain("text/plain");
    expect(contentType).toContain("version=");
    // gauge name = sanitize(lowercase("ns_count")); labels are the metric's dimensions.
    expect(body).toContain("# TYPE ns_count gauge");
    expect(body).toMatch(/ns_count\{[^}]*category="requests"[^}]*\}\s+5/);
    expect(body).toContain('coreName="thing-1"');
    expect(body).toContain('component="com.example.C"');
  });

  it("emitNow updates the registry too (latest value wins on a re-emit)", async () => {
    const t = track(await PrometheusTarget.create("ns", 0, "/metrics"));
    await t.emit(metric(), { count: 5 });
    await t.emitNow(metric(), { count: 9 });
    const { body } = await httpGet(t.port(), "/metrics");
    // Same label-set -> a single time series whose value is the latest (9, not 5).
    expect(body).toMatch(/ns_count\{[^}]*\}\s+9/);
    expect(body).not.toMatch(/ns_count\{[^}]*\}\s+5/);
  });

  it("flush() is a no-op w.r.t. delivery (no push; the scrape still pulls the current value)", async () => {
    const t = track(await PrometheusTarget.create("ns", 0, "/metrics"));
    await t.emit(metric(), { count: 7 });
    await expect(t.flush()).resolves.toBeUndefined();
    // After flush the endpoint still serves the in-process value (flush delivered nowhere).
    const { body } = await httpGet(t.port(), "/metrics");
    expect(body).toMatch(/ns_count\{[^}]*\}\s+7/);
  });

  it("close() stops the listener and releases the port", async () => {
    const t = await PrometheusTarget.create("ns", 0, "/metrics");
    const port = t.port();
    expect(port).toBeGreaterThan(0);
    await t.shutdown();
    // A GET to the released port now fails to connect.
    await expect(httpGet(port, "/metrics")).rejects.toBeTruthy();
    // shutdown is idempotent.
    await expect(t.shutdown()).resolves.toBeUndefined();
  });

  it("serves the configured non-default path; other paths 404 and non-GET 405", async () => {
    const t = track(await PrometheusTarget.create("ns", 0, "/prom"));
    await t.emit(metric(), { count: 1 });
    expect((await httpGet(t.port(), "/prom")).status).toBe(200);
    expect((await httpGet(t.port(), "/metrics")).status).toBe(404);

    const post = (): Promise<number> =>
      new Promise((resolve, reject) => {
        const req = http.request({ host: "127.0.0.1", port: t.port(), path: "/prom", method: "POST" }, (res) => {
          res.resume();
          res.on("end", () => resolve(res.statusCode ?? 0));
        });
        req.on("error", reject);
        req.end();
      });
    expect(await post()).toBe(405);
  });

  it("sanitizes hostile measure names and dimension keys in the exposition", async () => {
    const hostile = MetricBuilder.create("requests")
      .withThingName("thing-1")
      .withNamespace("ns")
      .addMeasure("Latency.ms-p99", "Milliseconds", 60)
      .addDimension("1region", "us-east-1")
      .addDimension("data center", "dc/1")
      .build();
    const t = track(await PrometheusTarget.create("ns", 0, "/metrics"));
    await t.emit(hostile, { "Latency.ms-p99": 42 });
    const { body } = await httpGet(t.port(), "/metrics");
    // measure name sanitized + lowercased: "ns_latency_ms_p99".
    expect(body).toContain("# TYPE ns_latency_ms_p99 gauge");
    // dimension key starting with a digit -> "_1region"; with a space -> "data_center". Values verbatim.
    expect(body).toContain('_1region="us-east-1"');
    expect(body).toContain('data_center="dc/1"');
    expect(body).toMatch(/ns_latency_ms_p99\{[^}]*\}\s+42/);
  });

  it("drops (does not widen) an emit whose label set differs from the gauge's first registration", async () => {
    // Canonical parity with Java/Python/Rust: the label set is fixed at first registration; a later
    // emit with a different label set is warned + skipped (NOT widened/reset).
    const t = track(await PrometheusTarget.create("ns", 0, "/metrics"));
    const base = MetricBuilder.create("requests").withNamespace("ns").addMeasure("count", "Count", 60).build();
    const extra = MetricBuilder.create("requests")
      .withNamespace("ns")
      .addMeasure("count", "Count", 60)
      .addDimension("region", "us-east-1")
      .build();
    await t.emit(base, { count: 1 }); // registers ns_count with the base label set
    await t.emit(extra, { count: 2 }); // adds `region` -> different label set -> dropped, no throw
    const { status, body } = await httpGet(t.port(), "/metrics");
    expect(status).toBe(200);
    // the mismatched emit was dropped: the value stays 1 and `region` never appears
    expect(body).not.toContain('region="us-east-1"');
    expect(body).toMatch(/ns_count\{[^}]*\}\s+1/);
  });

  it("ignores emits after shutdown (no throw)", async () => {
    const t = await PrometheusTarget.create("ns", 0, "/metrics");
    await t.shutdown();
    await expect(t.emit(metric(), { count: 1 })).resolves.toBeUndefined();
  });
});

describe("MetricEmitter selects the prometheus target", () => {
  it("metricEmission.target=prometheus builds a pull-based target served over HTTP", async () => {
    const port = await freePort();
    const config = Config.fromValue("com.example.C", "thing-1", {
      metricEmission: { target: "prometheus", namespace: "ns", targetConfig: { port, path: "/metrics" } },
    });
    const emitter = await MetricEmitter.create(config);
    const m = MetricBuilder.create("requests").withConfig(config).addMeasure("count", "Count", 60).build();
    emitter.defineMetric(m);
    await emitter.emitMetric("requests", { count: 3 });
    // emitMetricNow + flushMetrics push nowhere; the value is read by the scrape.
    await emitter.emitMetricNow("requests", { count: 4 });
    await emitter.flushMetrics();

    const { status, body } = await httpGet(port, "/metrics");
    expect(status).toBe(200);
    expect(body).toMatch(/ns_count\{[^}]*category="requests"[^}]*\}\s+4/);

    await emitter.shutdown();
    // The port is released after shutdown.
    await expect(httpGet(port, "/metrics")).rejects.toBeTruthy();
  });

  it("the platform-profile default selects prometheus when no explicit target is configured", async () => {
    const port = await freePort();
    // No metricEmission.target -> falls to the profile default passed as `targetDefault`.
    const config = Config.fromValue("com.example.C", "thing-1", {
      metricEmission: { namespace: "ns", targetConfig: { port, path: "/metrics" } },
    });
    const emitter = await MetricEmitter.create(config, undefined, "prometheus");
    const m = MetricBuilder.create("requests").withConfig(config).addMeasure("count", "Count", 60).build();
    emitter.defineMetric(m);
    await emitter.emitMetric("requests", { count: 8 });
    const { status, body } = await httpGet(port, "/metrics");
    expect(status).toBe(200);
    expect(body).toMatch(/ns_count\{[^}]*\}\s+8/);
    await emitter.shutdown();
  });

  it("an explicit config target overrides the prometheus profile default (log wins)", async () => {
    // target=log explicitly, even with a prometheus profile default -> NO http listener bound.
    const config = Config.fromValue("com.example.C", "thing-1", {
      metricEmission: { target: "log", namespace: "ns", targetConfig: { logFileName: "" } },
    });
    const emitter = await MetricEmitter.create(config, undefined, "prometheus");
    // A log target has no port; emit/flush/shutdown are all no-ops over a (here empty) file path.
    await expect(emitter.emitMetric("requests", { count: 1 })).resolves.toBeUndefined();
    await emitter.shutdown();
  });
});
