/**
 * StreamService / StreamHandle wrapper tests — the TS-side error/close/unknown-stream paths around
 * the native `streamlog-node` addon (which is built locally; buffer-only, no AWS).
 *
 * These exercise the JS wrapper logic the buffer-write happy-path test (streaming.test.ts) does not:
 * closed-handle/service guards, Uint8Array payload coercion, flush, idempotent close, the
 * stream()/stats() error translation seam, and the static streamNames() parser branches.
 */
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";

import { describe, expect, it } from "vitest";

import { EdgeStreamError, StreamHandle, StreamService } from "../src/streaming";

const ERR_UNKNOWN_STREAM = 5;

function tmpdir(): string {
  return fs.mkdtempSync(path.join(os.tmpdir(), "esl-ts-svc-"));
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

describe("StreamHandle close / closed guards", () => {
  it("append after close throws 'StreamHandle is closed'", () => {
    const svc = StreamService.open(config(tmpdir()));
    try {
      const h = svc.stream("telemetry");
      h.append("k", 1000, Buffer.from("v"));
      h.close();
      expect(() => h.append("k", 1001, Buffer.from("v"))).toThrow("StreamHandle is closed");
    } finally {
      svc.close();
    }
  });

  it("flush after close throws 'StreamHandle is closed'", () => {
    const svc = StreamService.open(config(tmpdir()));
    try {
      const h = svc.stream("telemetry");
      h.close();
      expect(() => h.flush()).toThrow("StreamHandle is closed");
    } finally {
      svc.close();
    }
  });

  it("close is idempotent (second close is a no-op)", () => {
    const svc = StreamService.open(config(tmpdir()));
    try {
      const h = svc.stream("telemetry");
      h.close();
      expect(() => h.close()).not.toThrow();
    } finally {
      svc.close();
    }
  });

  it("flush on an open handle succeeds (no throw)", () => {
    const svc = StreamService.open(config(tmpdir()));
    try {
      const h = svc.stream("telemetry");
      h.append("k", 1000, Buffer.from("v"));
      expect(() => h.flush()).not.toThrow();
    } finally {
      svc.close();
    }
  });

  it("a never-opened (null inner) handle reports closed", () => {
    // The constructor is public; a handle wrapping a null inner behaves exactly like a closed one.
    const h = new StreamHandle(null, "ghost");
    expect(h.name).toBe("ghost");
    expect(() => h.append("k", 1, Buffer.from("v"))).toThrow("StreamHandle is closed");
    expect(() => h.flush()).toThrow("StreamHandle is closed");
  });
});

describe("StreamHandle payload coercion", () => {
  it("accepts a Uint8Array payload (coerced to a Buffer) as well as a Buffer", () => {
    const svc = StreamService.open(config(tmpdir()));
    try {
      const h = svc.stream("telemetry");
      // Non-Buffer Uint8Array branch: must be wrapped in Buffer.from before crossing the FFI seam.
      h.append("k", 1000, new Uint8Array([1, 2, 3]));
      h.append("k", 1001, Buffer.from("buf"));
      h.flush();
      expect(svc.stats("telemetry").appendedTotal).toBe(2);
    } finally {
      svc.close();
    }
  });
});

describe("StreamService closed guards", () => {
  it("stream() after close throws 'StreamService is closed'", () => {
    const svc = StreamService.open(config(tmpdir()));
    svc.close();
    expect(() => svc.stream("telemetry")).toThrow("StreamService is closed");
  });

  it("stats() after close throws 'StreamService is closed'", () => {
    const svc = StreamService.open(config(tmpdir()));
    svc.close();
    expect(() => svc.stats("telemetry")).toThrow("StreamService is closed");
  });

  it("close is idempotent", () => {
    const svc = StreamService.open(config(tmpdir()));
    svc.close();
    expect(() => svc.close()).not.toThrow();
  });
});

describe("StreamService.stream() error translation", () => {
  it("requesting an unknown stream translates to EdgeStreamError ERR_UNKNOWN_STREAM", () => {
    const svc = StreamService.open(config(tmpdir()));
    try {
      // stream() (not just stats()) goes through translate() on the native ERR_UNKNOWN_STREAM.
      expect(() => svc.stream("nope")).toThrow(EdgeStreamError);
      try {
        svc.stream("nope");
        expect.unreachable("should have thrown");
      } catch (e) {
        expect(e).toBeInstanceOf(EdgeStreamError);
        expect((e as EdgeStreamError).code).toBe(ERR_UNKNOWN_STREAM);
      }
    } finally {
      svc.close();
    }
  });
});

describe("StreamService.streamNames parser branches", () => {
  it("returns the declared names for a valid config", () => {
    expect(StreamService.streamNames(config(tmpdir()))).toEqual(["telemetry"]);
  });

  it("returns [] for invalid JSON", () => {
    expect(StreamService.streamNames("{ not json")).toEqual([]);
  });

  it("returns [] when there is no streams array", () => {
    expect(StreamService.streamNames(JSON.stringify({ other: 1 }))).toEqual([]);
  });

  it("returns [] when streams is not an array", () => {
    expect(StreamService.streamNames(JSON.stringify({ streams: "x" }))).toEqual([]);
  });

  it("returns [] for a JSON null document", () => {
    // JSON.parse("null") => null; the `!doc` guard must short-circuit.
    expect(StreamService.streamNames("null")).toEqual([]);
  });

  it("filters out stream entries without a string name", () => {
    const cfg = JSON.stringify({
      streams: [{ name: "a" }, { name: 42 }, { nope: true }, { name: "b" }],
    });
    expect(StreamService.streamNames(cfg)).toEqual(["a", "b"]);
  });
});
