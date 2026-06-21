/**
 * Native streaming binding tests (napi-rs addon `ggstreamlog-node`). Requires the addon to be
 * built (`npm run build` in libs/rust-streamlog/bindings/node); buffer-only — no AWS needed.
 * Mirrors the Java/Python/Rust streaming tests.
 */
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";

import { describe, expect, it, vi } from "vitest";

import { Config } from "../src/config/model";
import type { MetricService } from "../src/metrics/types";
import { GgStreamError, StreamMetricsBridge, StreamService } from "../src/streaming";

const ERR_CONFIG = 1;
const ERR_UNKNOWN_STREAM = 5;

function tmpdir(): string {
  return fs.mkdtempSync(path.join(os.tmpdir(), "ggsl-ts-"));
}

function config(dir: string): string {
  return JSON.stringify({
    streams: [
      {
        name: "telemetry",
        sink: { type: "kinesis", streamName: "x" },
        buffer: {
          path: path.join(dir, "telemetry").replace(/\\/g, "/"),
          segmentBytes: 65536,
          maxDiskBytes: 1073741824,
          onFull: "block",
        },
      },
    ],
  });
}

describe("streaming native binding", () => {
  it("open / append / flush / stats", () => {
    const svc = StreamService.open(config(tmpdir()));
    const h = svc.stream("telemetry");
    for (let i = 0; i < 1000; i++) h.append("pump-7", 1000 + i, Buffer.from(`reading-${i}`));
    h.flush();
    const s = svc.stats("telemetry");
    expect(s.appendedTotal).toBe(1000);
    expect(s.nextOffset).toBe(1000);
    expect(s.backlog).toBe(1000); // buffer-only: nothing exported
    expect(s.droppedTotal).toBe(0); // block policy never drops
    expect(s.diskBytes).toBeGreaterThan(0);
    svc.close();
  });

  it("unknown stream reports ERR_UNKNOWN_STREAM", () => {
    const svc = StreamService.open(config(tmpdir()));
    try {
      svc.stats("does-not-exist");
      expect.unreachable("should have thrown");
    } catch (e) {
      expect(e).toBeInstanceOf(GgStreamError);
      expect((e as GgStreamError).code).toBe(ERR_UNKNOWN_STREAM);
    } finally {
      svc.close();
    }
  });

  it("bad config reports ERR_CONFIG", () => {
    try {
      StreamService.open("{ not valid json");
      expect.unreachable("should have thrown");
    } catch (e) {
      expect((e as GgStreamError).code).toBe(ERR_CONFIG);
    }
  });

  it("streamNames parses the config", () => {
    expect(StreamService.streamNames(config(tmpdir()))).toEqual(["telemetry"]);
  });

  it("metrics bridge defines + emits per stream", async () => {
    const cfg = Config.fromValue("comp", "thing", {});
    const emitted: Array<[string, Record<string, number>]> = [];
    const metrics: MetricService = {
      defineMetric: vi.fn(),
      isMetricDefined: () => true,
      emitMetric: async (n, v) => {
        emitted.push([n, v]);
      },
      emitMetricNow: async () => undefined,
      flushMetrics: async () => undefined,
      shutdown: async () => undefined,
    };

    const svc = StreamService.open(config(tmpdir()));
    const h = svc.stream("telemetry");
    for (let i = 0; i < 10; i++) h.append("k", 1000 + i, Buffer.from("v"));
    h.flush();

    const bridge = new StreamMetricsBridge(cfg, metrics, svc, ["telemetry"], 1);
    try {
      expect(metrics.defineMetric).toHaveBeenCalledTimes(1);
      await vi.waitFor(() => expect(emitted.length).toBeGreaterThan(0), { timeout: 4000 });
      expect(emitted[0][0]).toBe("stream:telemetry");
      expect(emitted[0][1]).toHaveProperty("backlog");
    } finally {
      bridge.close();
      svc.close();
    }
  });
});
