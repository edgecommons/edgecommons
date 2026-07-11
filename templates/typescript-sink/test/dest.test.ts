import { promises as fs } from "node:fs";
import * as os from "node:os";
import * as path from "node:path";

import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { DeliverError, Delivered, Item, LocalDestination, buildDestination } from "../src/dest";

const item = (key: string, body: string): Item => ({ key, bytes: Buffer.from(body, "utf8") });

describe("the local destination", () => {
  let root: string;

  beforeEach(async () => {
    root = await fs.mkdtemp(path.join(os.tmpdir(), "sink-"));
  });
  afterEach(async () => {
    await fs.rm(root, { recursive: true, force: true });
  });

  it("lands the object at its stable key", async () => {
    const dest = new LocalDestination(root);
    const it0 = item("a/b/thing.json", "hello");

    const got = await dest.deliver(it0);
    expect(got.bytesWritten).toBe(5);
    await dest.verify(it0, got); // resolves

    expect(await fs.readFile(path.join(root, "a/b/thing.json"), "utf8")).toBe("hello");
  });

  it("overwrites on redelivery rather than duplicating", async () => {
    // This is what makes retry safe. If a redelivery could duplicate, a sink could not retry.
    const dest = new LocalDestination(root);

    await dest.deliver(item("thing.json", "first"));
    const second = item("thing.json", "second");
    const got = await dest.deliver(second);
    await dest.verify(second, got);

    expect(await fs.readFile(path.join(root, "thing.json"), "utf8")).toBe("second");
    expect(await fs.readdir(root)).toEqual(["thing.json"]); // one object, not two
  });

  it("leaves no partial file behind — the rename is the atomic commit", async () => {
    const dest = new LocalDestination(root);
    await dest.deliver(item("thing.json", "hello"));

    const leftovers = (await fs.readdir(root)).filter((name) => name.includes("partial"));
    expect(leftovers).toEqual([]);
  });

  it("refuses a mismatch in verify, so the source is never released", async () => {
    const dest = new LocalDestination(root);
    const it0 = item("thing.json", "hello");
    await dest.deliver(it0);

    // Claim we wrote more than we did: verify must catch it.
    const lying: Delivered = { bytesWritten: 999 };
    await expect(dest.verify(it0, lying)).rejects.toThrow(/size mismatch/);
  });

  it("fails verify when the object is not there at all", async () => {
    const dest = new LocalDestination(root);
    await expect(dest.verify(item("missing.json", ""), { bytesWritten: 0 })).rejects.toThrow(/stat/);
  });
});

describe("error classification", () => {
  it("decides whether retrying can help", () => {
    expect(DeliverError.isTransient(DeliverError.transientError("timeout"))).toBe(true);
    expect(DeliverError.isTransient(DeliverError.permanent("bad credentials"))).toBe(false);
    // An unclassified throw is treated as transient: a wrongly-permanent verdict loses data.
    expect(DeliverError.isTransient(new Error("boom"))).toBe(true);
  });
});

describe("destination config", () => {
  it("builds a destination from config", () => {
    expect(buildDestination({ type: "local", path: "/tmp/out" }).kind).toBe("local");
  });

  it("rejects an unknown destination, or one missing its required key", () => {
    expect(() => buildDestination({ type: "s3", bucket: "b" })).toThrow(/unknown destination/);
    expect(() => buildDestination({ type: "local" })).toThrow(/path/);
    expect(() => buildDestination({ type: "local", path: "/tmp", pth: "x" })).toThrow(/unknown key/);
  });
});
