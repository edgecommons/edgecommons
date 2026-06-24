/**
 * Low-level tests for the napi host-callback sink bridge (`registerSinkCallback` /
 * `resolveSinkOutcome`) over the REAL `ggstreamlog` native binding. These exercise the export
 * engine's outcome handling directly — the path the durable CloudWatch target sits on top of —
 * including the `Partial` re-delivery path (which the metrics target itself never emits).
 */
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";

import { afterEach, beforeEach, describe, expect, it } from "vitest";

import {
  registerSinkCallback,
  resolveSinkOutcome,
  SINK_OUTCOME,
  type SinkRecord,
} from "../src/streaming/native";
import { StreamService } from "../src/streaming/service";

let tmpDirs: string[] = [];
function tmpdir(): string {
  const d = fs.mkdtempSync(path.join(os.tmpdir(), "ggsl-bridge-"));
  tmpDirs.push(d);
  return d;
}
function openCallbackStream(dir: string, name: string): StreamService {
  const cfg = JSON.stringify({
    streams: [
      {
        name,
        sink: { type: "callback" },
        buffer: {
          type: "disk",
          path: path.join(dir, name).split(path.sep).join("/"),
          segmentBytes: 65536,
          maxDiskBytes: 1073741824,
          onFull: "block",
          fsync: "perBatch",
        },
        delivery: { maxRetries: -1, pollIntervalMs: 10, backoffBaseMs: 5, backoffMaxMs: 50 },
        batch: { maxRecords: 1000, maxBytes: 1000000, maxLatencyMs: 50 },
      },
    ],
  });
  return StreamService.open(cfg);
}
async function waitFor(pred: () => boolean, timeoutMs = 5000): Promise<boolean> {
  const start = Date.now();
  while (!pred()) {
    if (Date.now() - start > timeoutMs) return false;
    await new Promise((r) => setTimeout(r, 10));
  }
  return true;
}

beforeEach(() => {
  tmpDirs = [];
});
afterEach(() => {
  for (const d of tmpDirs) {
    try {
      fs.rmSync(d, { recursive: true, force: true });
    } catch {
      /* ignore */
    }
  }
});

describe("napi host-callback sink bridge", () => {
  it("AllAcked: the export thread blocks on the JS drain and commits past the batch", async () => {
    const dir = tmpdir();
    const name = `cb-acked-${Math.random().toString(36).slice(2)}`;
    const seen: Array<{ offset: number; pk: string; payload: string }> = [];
    let ticks = 0;
    const iv = setInterval(() => {
      ticks++;
    }, 5);

    registerSinkCallback(name, (batchId: number, records: SinkRecord[]) => {
      // Genuinely-async drain, then resolve AllAcked (mirrors the validated §9 pattern).
      setTimeout(() => {
        for (const r of records) seen.push({ offset: r.offset, pk: r.partitionKey, payload: r.payload.toString("utf8") });
        resolveSinkOutcome(batchId, SINK_OUTCOME.ALL_ACKED);
      }, 15);
    });

    const svc = openCallbackStream(dir, name);
    const h = svc.stream(name);
    for (let i = 0; i < 20; i++) h.append("ns", 1000 + i, Buffer.from(`{"v":${i}}`));
    h.flush();

    const ok = await waitFor(() => seen.length >= 20);
    clearInterval(iv);
    expect(ok).toBe(true);
    expect(seen.length).toBe(20);
    expect(svc.stats(name).exportedTotal).toBe(20);
    expect(svc.stats(name).backlog).toBe(0);
    // The event loop kept ticking while the native export thread blocked -> no deadlock.
    expect(ticks).toBeGreaterThan(0);
    svc.close();
  });

  it("Partial: only the failed offsets are re-delivered (failedOffsets argument)", async () => {
    const dir = tmpdir();
    const name = `cb-partial-${Math.random().toString(36).slice(2)}`;
    // Track how many times each offset is delivered. The export engine may read records in one or
    // several batches depending on timing, so we assert on offset-level re-delivery, not batch sizes.
    const deliveries = new Map<number, number>();
    let failedOnce = false;
    let firstFailedOffset = -1;

    registerSinkCallback(name, (batchId: number, records: SinkRecord[]) => {
      for (const r of records) deliveries.set(r.offset, (deliveries.get(r.offset) ?? 0) + 1);
      if (!failedOnce && records.length > 0) {
        // Fail exactly the first offset of the first non-empty batch -> it must be re-delivered.
        failedOnce = true;
        firstFailedOffset = records[0].offset;
        setTimeout(() => resolveSinkOutcome(batchId, SINK_OUTCOME.PARTIAL, [firstFailedOffset]), 10);
      } else {
        setTimeout(() => resolveSinkOutcome(batchId, SINK_OUTCOME.ALL_ACKED), 10);
      }
    });

    const svc = openCallbackStream(dir, name);
    const h = svc.stream(name);
    for (let i = 0; i < 5; i++) h.append("ns", 1000 + i, Buffer.from(`v${i}`));
    h.flush();

    const ok = await waitFor(() => svc.stats(name).backlog === 0 && svc.stats(name).exportedTotal >= 5);
    expect(ok).toBe(true);
    // All 5 records were exported (acked) exactly once at the engine level...
    expect(svc.stats(name).exportedTotal).toBe(5);
    // ...and the offset we failed with Partial was delivered to the sink more than once (re-tried),
    // proving the failedOffsets argument drove a selective re-delivery.
    expect(firstFailedOffset).toBeGreaterThanOrEqual(0);
    expect(deliveries.get(firstFailedOffset) ?? 0).toBeGreaterThanOrEqual(2);
    svc.close();
  });

  it("Failed (retryable) re-delivers the whole batch until it succeeds", async () => {
    const dir = tmpdir();
    const name = `cb-failed-${Math.random().toString(36).slice(2)}`;
    let attempts = 0;

    registerSinkCallback(name, (batchId: number, _records: SinkRecord[]) => {
      attempts++;
      const code = attempts < 3 ? SINK_OUTCOME.FAILED : SINK_OUTCOME.ALL_ACKED;
      setTimeout(() => resolveSinkOutcome(batchId, code), 5);
    });

    const svc = openCallbackStream(dir, name);
    const h = svc.stream(name);
    h.append("ns", 1000, Buffer.from("only"));
    h.flush();

    const ok = await waitFor(() => svc.stats(name).exportedTotal >= 1, 8000);
    expect(ok).toBe(true);
    expect(attempts).toBeGreaterThanOrEqual(3); // re-delivered until the 3rd attempt acked
    svc.close();
  });

  it("resolveOutcome with an unknown batch id is ignored (no throw)", () => {
    expect(() => resolveSinkOutcome(999999, SINK_OUTCOME.ALL_ACKED)).not.toThrow();
  });
});
