/**
 * Native streaming binding tests (napi-rs addon `streamlog-node`). Requires the addon to be
 * built (`npm run build` in libs/rust-streamlog/bindings/node); buffer-only — no AWS needed.
 * Mirrors the Java/Python/Rust streaming tests.
 */
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";

import { describe, expect, it, vi } from "vitest";

import { Config } from "../src/config/model";
import type { MetricService } from "../src/metrics/types";
import { EdgeStreamError, StreamMetricsBridge, StreamService } from "../src/streaming";

const ERR_CONFIG = 1;
const ERR_UNKNOWN_STREAM = 5;

function tmpdir(): string {
  return fs.mkdtempSync(path.join(os.tmpdir(), "esl-ts-"));
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
      expect(e).toBeInstanceOf(EdgeStreamError);
      expect((e as EdgeStreamError).code).toBe(ERR_UNKNOWN_STREAM);
    } finally {
      svc.close();
    }
  });

  it("bad config reports ERR_CONFIG", () => {
    try {
      StreamService.open("{ not valid json");
      expect.unreachable("should have thrown");
    } catch (e) {
      expect((e as EdgeStreamError).code).toBe(ERR_CONFIG);
    }
  });

  it("streamNames parses the config", () => {
    expect(StreamService.streamNames(config(tmpdir()))).toEqual(["telemetry"]);
  });

  // A store that round-trips JSON numbers as doubles (the Greengrass Nucleus) delivers `65536.0`.
  // JavaScript cannot express that distinction through JSON.stringify — the wire text is what
  // carries it — so these build the document as text, the way the host receives it. The native
  // binding canonicalizes the numbers at intake.
  function storeShapedConfig(dir: string, segmentBytes: string): string {
    const bufferPath = path.join(dir, "telemetry").replace(/\\/g, "/");
    return `{"streams":[{"name":"telemetry",
      "sink":{"type":"kinesis","streamName":"x"},
      "buffer":{"path":"${bufferPath}","segmentBytes":${segmentBytes},
                "maxDiskBytes":1073741824.0,"maxAgeSecs":3600.0,
                "fsyncIntervalMs":1000.0,"maxBufferedRecords":128.0,"onFull":"block"},
      "batch":{"maxRecords":500.0,"maxBytes":4194304.0},
      "delivery":{"maxRetries":-1.0,"pollIntervalMs":1000.0}}]}`;
  }

  it("opens a store-shaped config whose numbers are doubles", () => {
    const svc = StreamService.open(storeShapedConfig(tmpdir(), "65536.0"));
    try {
      const h = svc.stream("telemetry");
      h.append("pump-7", 1000, Buffer.from("reading"));
      h.flush();
      expect(svc.stats("telemetry").appendedTotal).toBe(1);
    } finally {
      svc.close();
    }
  });

  it("refuses a fractional value instead of truncating it", () => {
    try {
      StreamService.open(storeShapedConfig(tmpdir(), "65536.5"));
      expect.unreachable("should have thrown");
    } catch (e) {
      expect((e as EdgeStreamError).code).toBe(ERR_CONFIG);
    }
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
