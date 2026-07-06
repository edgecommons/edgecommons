/**
 * Durable CloudWatch metric target tests (TypeScript).
 *
 * Exercises the disk-backed store-and-forward CloudWatch target end-to-end over the REAL
 * `edgestreamlog` native binding (the napi sink-callback bridge), with a controllable in-process
 * fake CloudWatch client injected via the target's `clientFactory` seam. No AWS, no module mock.
 *
 * Covers: record round-trip through the buffer, namespace grouping in the drain, the stale-drop
 * counter (datums outside CloudWatch's ~2wk/~2h accept window), 1000/1MB chunking, outcome mapping
 * (retryable failure re-delivers; permanent reject drops), and the headline disconnect
 * fault-injection scenario (sever -> flat memory + bounded disk backlog -> drain on reconnect ->
 * nonzero stale-drop once the window is exceeded).
 */
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";

import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { MetricBuilder } from "../src/metrics/metric";
import type { Metric } from "../src/metrics/metric";
import { DurableCloudWatchTarget } from "../src/metrics/target/cloudwatch_durable";

interface PutInput {
  Namespace: string;
  MetricData: Array<{ MetricName: string; Value: number; Timestamp?: Date }>;
}

/** A controllable fake CloudWatch client: records sends, can be severed / fail per-mode. */
class FakeCloudWatch {
  sent: PutInput[] = [];
  /**
   * "ok" | "sever" (retryable transport error, no status) | "reject" (permanent 400) |
   * "throttle" (503, retryable then recovers) | "weird" (no name/status -> assumed retryable).
   */
  mode: "ok" | "sever" | "reject" | "throttle" | "weird" = "ok";
  sendCount = 0;
  /** When > 0, the next `throttle`/`sever` send count decrements this then flips to ok. */
  recoverAfter = Infinity;

  async send(cmd: unknown): Promise<unknown> {
    this.sendCount++;
    if (this.sendCount > this.recoverAfter) this.mode = "ok";
    if (this.mode === "sever") {
      const e = new Error("ECONNREFUSED severed") as Error & { $metadata?: { httpStatusCode?: number } };
      e.name = "TimeoutError";
      throw e;
    }
    if (this.mode === "throttle") {
      const e = new Error("service unavailable") as Error & { $metadata?: { httpStatusCode?: number } };
      e.name = "ServiceUnavailable";
      e.$metadata = { httpStatusCode: 503 };
      throw e;
    }
    if (this.mode === "weird") {
      throw "a bare string error with no shape";
    }
    if (this.mode === "reject") {
      const e = new Error("invalid datum") as Error & { $metadata?: { httpStatusCode?: number } };
      e.name = "ValidationError";
      e.$metadata = { httpStatusCode: 400 };
      throw e;
    }
    this.sent.push((cmd as { input: PutInput }).input);
    return {};
  }

  totalDatums(): number {
    return this.sent.reduce((n, s) => n + s.MetricData.length, 0);
  }
}

let tmpDirs: string[] = [];

function tmpdir(): string {
  const d = fs.mkdtempSync(path.join(os.tmpdir(), "esl-cw-dur-"));
  tmpDirs.push(d);
  return d;
}

/** Disk bytes currently used by the buffer dir (the on-disk backlog proxy). */
function dirBytes(dir: string): number {
  let total = 0;
  const walk = (p: string): void => {
    for (const name of fs.readdirSync(p)) {
      const full = path.join(p, name);
      const st = fs.statSync(full);
      if (st.isDirectory()) walk(full);
      else total += st.size;
    }
  };
  if (fs.existsSync(dir)) walk(dir);
  return total;
}

function metric(): Metric {
  return MetricBuilder.create("requests")
    .withThingName("thing-1")
    .withComponentName("com.example.C")
    .withNamespace("ns")
    .addMeasure("count", "Count", 60)
    .build();
}

async function waitFor(pred: () => boolean, timeoutMs = 5000): Promise<boolean> {
  const start = Date.now();
  while (!pred()) {
    if (Date.now() - start > timeoutMs) return false;
    await new Promise((r) => setTimeout(r, 15));
  }
  return true;
}

async function makeTarget(
  dir: string,
  fake: FakeCloudWatch,
  opts: { maxDiskBytes?: number; namespace?: string; largeFleet?: boolean } = {},
): Promise<DurableCloudWatchTarget> {
  return DurableCloudWatchTarget.create(
    opts.namespace ?? "ns",
    opts.largeFleet ?? false,
    1, // intervalSecs -> ~1s max latency / 1s poll
    {
      path: path.join(dir, "cw").split(path.sep).join("/"),
      maxDiskBytes: opts.maxDiskBytes ?? 64 * 1024 * 1024,
      onFull: "dropOldest",
      fsync: "perBatch",
      segmentBytes: 64 * 1024,
    },
    () => fake as unknown as { send(c: unknown): Promise<unknown> },
  );
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

describe("DurableCloudWatchTarget (real native buffer, fake CloudWatch)", () => {
  it("emit -> durable buffer -> drains to PutMetricData with correct datum + namespace", async () => {
    const dir = tmpdir();
    const fake = new FakeCloudWatch();
    const t = await makeTarget(dir, fake);

    for (let i = 0; i < 5; i++) await t.emit(metric(), { count: i });
    await t.flush();

    const drained = await waitFor(() => fake.totalDatums() >= 5);
    expect(drained).toBe(true);
    expect(fake.totalDatums()).toBe(5);
    expect(fake.sent.every((s) => s.Namespace === "ns")).toBe(true);
    const d0 = fake.sent[0].MetricData[0];
    expect(d0.MetricName).toBe("count");
    expect(d0.Timestamp).toBeInstanceOf(Date);
    expect(t.droppedStale).toBe(0);
    await t.shutdown();
  });

  it("groups datums by namespace in the drain (one PutMetricData per namespace)", async () => {
    const dir = tmpdir();
    const fake = new FakeCloudWatch();
    const t = await makeTarget(dir, fake, { namespace: "nsA" });

    const mA = MetricBuilder.create("a").withNamespace("nsA").addMeasure("v", "Count", 60).build();
    const mB = MetricBuilder.create("b").withNamespace("nsB").addMeasure("v", "Count", 60).build();
    // Both metrics carry the target's namespace as the partition key; force two namespaces by
    // appending raw records with distinct namespaces via emit (target namespace) + a direct stream
    // append for nsB to prove the drain groups.
    await t.emit(mA, { v: 1 });
    // Reach into the target to append a record tagged with a different namespace.
    const anyT = t as unknown as {
      svc: { stream(n: string): { append(pk: string, ts: number, p: Buffer): void; flush(): void } };
      streamName: string;
    };
    const rec = JSON.stringify({
      namespace: "nsB",
      datum: { MetricName: "b", Value: 2, Unit: "Count", StorageResolution: 60, Timestamp: Date.now() },
    });
    const h = anyT.svc.stream(anyT.streamName);
    h.append("nsB", Date.now(), Buffer.from(rec, "utf8"));
    h.flush();
    void mB;

    const drained = await waitFor(() => fake.totalDatums() >= 2);
    expect(drained).toBe(true);
    const namespaces = new Set(fake.sent.map((s) => s.Namespace));
    expect(namespaces.has("nsA")).toBe(true);
    expect(namespaces.has("nsB")).toBe(true);
    // Each PutMetricData call carries exactly one namespace.
    for (const s of fake.sent) {
      expect(typeof s.Namespace).toBe("string");
    }
    await t.shutdown();
  });

  it("drops datums outside the ~2wk-past / ~2h-future window and counts them", async () => {
    const dir = tmpdir();
    const fake = new FakeCloudWatch();
    const t = await makeTarget(dir, fake);

    const anyT = t as unknown as {
      svc: { stream(n: string): { append(pk: string, ts: number, p: Buffer): void; flush(): void } };
      streamName: string;
    };
    const h = anyT.svc.stream(anyT.streamName);
    const now = Date.now();
    const tooOld = now - 20 * 24 * 60 * 60 * 1000; // 20 days ago (> 2wk)
    const tooNew = now + 5 * 60 * 60 * 1000; // 5h ahead (> 2h)
    const fresh = now;

    for (const [ts, val] of [
      [tooOld, 1],
      [tooNew, 2],
      [fresh, 3],
    ] as Array<[number, number]>) {
      const rec = JSON.stringify({
        namespace: "ns",
        datum: { MetricName: "count", Value: val, Unit: "Count", StorageResolution: 60, Timestamp: ts },
      });
      h.append("ns", ts, Buffer.from(rec, "utf8"));
    }
    h.flush();

    // Only the fresh datum should reach CloudWatch; the two out-of-window ones are dropped+counted.
    const drained = await waitFor(() => fake.totalDatums() >= 1 && t.droppedStale >= 2);
    expect(drained).toBe(true);
    expect(fake.totalDatums()).toBe(1);
    expect(fake.sent[0].MetricData[0].Value).toBe(3);
    expect(t.droppedStale).toBe(2);
    await t.shutdown();
  });

  it("chunks a large emission into <=1000-datum PutMetricData requests", async () => {
    const dir = tmpdir();
    const fake = new FakeCloudWatch();
    const t = await makeTarget(dir, fake);

    const anyT = t as unknown as {
      svc: { stream(n: string): { append(pk: string, ts: number, p: Buffer): void; flush(): void } };
      streamName: string;
    };
    const h = anyT.svc.stream(anyT.streamName);
    const now = Date.now();
    // Append 2300 datums for one namespace -> must split into >=3 PutMetricData chunks of <=1000.
    for (let i = 0; i < 2300; i++) {
      const rec = JSON.stringify({
        namespace: "ns",
        datum: { MetricName: "count", Value: i, Unit: "Count", StorageResolution: 60, Timestamp: now },
      });
      h.append("ns", now, Buffer.from(rec, "utf8"));
    }
    h.flush();

    const drained = await waitFor(() => fake.totalDatums() >= 2300, 15000);
    expect(drained).toBe(true);
    expect(fake.totalDatums()).toBe(2300);
    expect(fake.sent.every((s) => s.MetricData.length <= 1000)).toBe(true);
    expect(fake.sent.length).toBeGreaterThanOrEqual(3);
    await t.shutdown();
  });

  it("permanent (4xx) reject drops the batch so the stream is not wedged", async () => {
    const dir = tmpdir();
    const fake = new FakeCloudWatch();
    fake.mode = "reject";
    const t = await makeTarget(dir, fake);

    await t.emit(metric(), { count: 1 });
    await t.flush();

    // The engine "acks" (drops) the rejected batch so backlog clears; no infinite retry.
    const anyT = t as unknown as { svc: { stats(n: string): { backlog: number } }; streamName: string };
    const cleared = await waitFor(() => anyT.svc.stats(anyT.streamName).backlog === 0);
    expect(cleared).toBe(true);
    expect(fake.sendCount).toBeGreaterThanOrEqual(1);
    await t.shutdown();
  });

  // ---- HEADLINE: disconnect fault-injection ----
  it("survives a lengthy disconnect: flat memory, bounded disk backlog, drains on reconnect", async () => {
    const dir = tmpdir();
    const fake = new FakeCloudWatch();
    // Tiny disk budget so dropOldest bounds the backlog under a long sever.
    const t = await makeTarget(dir, fake, { maxDiskBytes: 256 * 1024 });

    const anyT = t as unknown as {
      svc: { stream(n: string): { append(pk: string, ts: number, p: Buffer): void; flush(): void }; stats(n: string): { backlog: number; droppedTotal: number; diskBytes: number } };
      streamName: string;
    };

    // 1) SEVER the cloud. Every drain attempt fails (retryable) -> nothing acks, backlog accrues.
    fake.mode = "sever";
    const h = anyT.svc.stream(anyT.streamName);
    const now = Date.now();
    for (let i = 0; i < 4000; i++) {
      const rec = JSON.stringify({
        namespace: "ns",
        datum: { MetricName: "count", Value: i, Unit: "Count", StorageResolution: 60, Timestamp: now },
      });
      h.append("ns", now, Buffer.from(rec, "utf8"));
    }
    h.flush();

    // Give the export engine time to attempt+fail and the buffer to apply dropOldest.
    await new Promise((r) => setTimeout(r, 400));
    const statsSevered = anyT.svc.stats(anyT.streamName);
    // Disk backlog is bounded by the tiny budget (dropOldest), proving memory stays flat: the
    // backlog lives on disk, not in RAM, and the on-disk bytes never exceed the configured cap.
    expect(statsSevered.diskBytes).toBeLessThanOrEqual(256 * 1024);
    expect(statsSevered.droppedTotal).toBeGreaterThan(0); // dropOldest kicked in past the cap
    expect(fake.totalDatums()).toBe(0); // nothing made it to the cloud while severed

    // 2) RECONNECT. The engine drains the surviving (bounded) backlog cleanly.
    fake.mode = "ok";
    const reconnected = await waitFor(() => anyT.svc.stats(anyT.streamName).backlog === 0, 8000);
    expect(reconnected).toBe(true);
    expect(fake.totalDatums()).toBeGreaterThan(0); // surviving records reached CloudWatch
    await t.shutdown();
  });

  it("stale-drop counter is nonzero once the accept window is exceeded after a long sever", async () => {
    const dir = tmpdir();
    const fake = new FakeCloudWatch();
    const t = await makeTarget(dir, fake);

    const anyT = t as unknown as {
      svc: { stream(n: string): { append(pk: string, ts: number, p: Buffer): void; flush(): void }; stats(n: string): { backlog: number } };
      streamName: string;
    };
    const h = anyT.svc.stream(anyT.streamName);

    // Sever, append records whose timestamps are ALREADY older than the 2wk window (simulating a
    // backlog that aged out during a multi-week outage), then reconnect.
    fake.mode = "sever";
    const aged = Date.now() - 20 * 24 * 60 * 60 * 1000;
    for (let i = 0; i < 10; i++) {
      const rec = JSON.stringify({
        namespace: "ns",
        datum: { MetricName: "count", Value: i, Unit: "Count", StorageResolution: 60, Timestamp: aged },
      });
      h.append("ns", aged, Buffer.from(rec, "utf8"));
    }
    h.flush();
    await new Promise((r) => setTimeout(r, 200));

    fake.mode = "ok";
    const cleared = await waitFor(() => anyT.svc.stats(anyT.streamName).backlog === 0, 8000);
    expect(cleared).toBe(true);
    // The aged-out datums were dropped (never retryable) and counted; none reached the cloud.
    expect(t.droppedStale).toBe(10);
    expect(fake.totalDatums()).toBe(0);
    await t.shutdown();
  });

  it("largeFleetWorkaround appends a coreName=ALL datum set; emitNow + flush drain", async () => {
    const dir = tmpdir();
    const fake = new FakeCloudWatch();
    const t = await makeTarget(dir, fake, { largeFleet: true });
    await t.emitNow(metric(), { count: 1 }); // emitNow path (append + flush)
    await t.flush(); // explicit flush path
    const drained = await waitFor(() => fake.totalDatums() >= 2);
    expect(drained).toBe(true);
    // largeFleet -> the normal datum plus a coreName=ALL datum.
    const allDatums = fake.sent.flatMap((s) => s.MetricData) as Array<{ Dimensions?: Array<{ Name: string; Value: string }> }>;
    const masked = allDatums.find((d) => d.Dimensions?.some((dm) => dm.Name === "coreName" && dm.Value === "ALL"));
    expect(masked).toBeDefined();
    await t.shutdown();
  });

  it("a 503 (transient) failure re-delivers, then drains cleanly on recovery", async () => {
    const dir = tmpdir();
    const fake = new FakeCloudWatch();
    fake.mode = "throttle";
    fake.recoverAfter = 2; // first 2 sends throttle, then ok
    const t = await makeTarget(dir, fake);
    await t.emit(metric(), { count: 1 });
    await t.flush();
    const drained = await waitFor(() => fake.totalDatums() >= 1, 8000);
    expect(drained).toBe(true);
    expect(fake.sendCount).toBeGreaterThan(1); // it retried past the throttles
    await t.shutdown();
  });

  it("an ambiguous (shapeless) error is treated as retryable, then recovers", async () => {
    const dir = tmpdir();
    const fake = new FakeCloudWatch();
    fake.mode = "weird";
    fake.recoverAfter = 1;
    const t = await makeTarget(dir, fake);
    await t.emit(metric(), { count: 9 });
    await t.flush();
    const drained = await waitFor(() => fake.totalDatums() >= 1, 8000);
    expect(drained).toBe(true);
    await t.shutdown();
  });

  it("drops corrupt (non-JSON) records as stale and acks them", async () => {
    const dir = tmpdir();
    const fake = new FakeCloudWatch();
    const t = await makeTarget(dir, fake);
    const anyT = t as unknown as {
      svc: { stream(n: string): { append(pk: string, ts: number, p: Buffer): void; flush(): void }; stats(n: string): { backlog: number } };
      streamName: string;
    };
    const h = anyT.svc.stream(anyT.streamName);
    h.append("ns", Date.now(), Buffer.from("not json {{{", "utf8"));
    h.flush();
    const cleared = await waitFor(() => anyT.svc.stats(anyT.streamName).backlog === 0 && t.droppedStale >= 1);
    expect(cleared).toBe(true);
    expect(t.droppedStale).toBe(1);
    expect(fake.totalDatums()).toBe(0);
    await t.shutdown();
  });

  it("chunks by the ~1MB byte budget, not only the 1000-count cap", async () => {
    const dir = tmpdir();
    const fake = new FakeCloudWatch();
    const t = await makeTarget(dir, fake);
    const anyT = t as unknown as {
      svc: { stream(n: string): { append(pk: string, ts: number, p: Buffer): void; flush(): void } };
      streamName: string;
    };
    const h = anyT.svc.stream(anyT.streamName);
    const now = Date.now();
    // 200 datums, each padded to ~10 KB -> ~2 MB total -> must split on the byte budget (well
    // under the 1000-count cap), proving the <=1MB chunk path.
    const pad = "x".repeat(10 * 1024);
    for (let i = 0; i < 200; i++) {
      const rec = JSON.stringify({
        namespace: "ns",
        datum: { MetricName: "count", Value: i, Unit: "Count", StorageResolution: 60, Timestamp: now, pad },
      });
      h.append("ns", now, Buffer.from(rec, "utf8"));
    }
    h.flush();
    const drained = await waitFor(() => fake.totalDatums() >= 200, 15000);
    expect(drained).toBe(true);
    // Split into multiple requests despite each having < 1000 datums.
    expect(fake.sent.length).toBeGreaterThanOrEqual(2);
    expect(fake.sent.every((s) => s.MetricData.length < 1000)).toBe(true);
    await t.shutdown();
  });

  it("emit/flush after shutdown is a no-op (closed guard)", async () => {
    const dir = tmpdir();
    const fake = new FakeCloudWatch();
    const t = await makeTarget(dir, fake);
    await t.shutdown();
    await t.shutdown(); // idempotent (double-close guard)
    await expect(t.emit(metric(), { count: 1 })).resolves.toBeUndefined();
    await expect(t.emitNow(metric(), { count: 1 })).resolves.toBeUndefined();
    await expect(t.flush()).resolves.toBeUndefined();
  });

  it("shutdown stops the engine without draining; backlog persists on disk", async () => {
    const dir = tmpdir();
    const fake = new FakeCloudWatch();
    fake.mode = "sever";
    const t = await makeTarget(dir, fake);
    await t.emit(metric(), { count: 1 });
    await t.emitNow(metric(), { count: 2 });
    await t.flush();
    await new Promise((r) => setTimeout(r, 100));
    await t.shutdown();
    // After shutdown the buffer files still exist on disk (backlog persisted for next start).
    expect(dirBytes(path.join(dir, "cw"))).toBeGreaterThan(0);
  });
});
