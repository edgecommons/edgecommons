/**
 * Unit tests for {@link ConfigMapConfigSource} (the `-c CONFIGMAP` k8s-native source): config load
 * from a mounted-style temp directory, the kubelet dotfile filter (FR-CFG-4), reject-and-keep on an
 * invalid reload (FR-CFG-5), the subPath warning detection (FR-CFG-3), and the directory-watch
 * RE-ARM verified both by a deterministic late-mount retry and by simulating the kubelet atomic
 * `..data` symlink swap (FR-CFG-2). Mirrors the canonical Java `ConfigMapConfigProviderTest` +
 * `DirectoryWatcherTest`.
 *
 * The swap test needs OS symlink support (skipped on Windows without privilege, like the Java
 * `assumeTrue`); the rest run on every OS. The full end-to-end `..data` hot-reload is verified live
 * on kind by the orchestrator.
 */
import { describe, it, expect, afterEach, vi } from "vitest";
import * as fs from "fs";
import * as fsp from "fs/promises";
import * as os from "os";
import * as path from "path";

import { EdgeCommonsError } from "../src/errors";
import { logger } from "../src/logging";
import { ConfigMapConfigSource } from "../src/config/source/configmap";
import { buildConfigSource } from "../src/config/source";
import { isProjectionArtifact } from "../src/parameters/source";

const dirs: string[] = [];
function tmpDir(): string {
  const d = fs.mkdtempSync(path.join(os.tmpdir(), "ggc-cm-"));
  dirs.push(d);
  return d;
}
function configJson(version: number): string {
  return JSON.stringify({ component: { name: "x" }, version });
}
const sleep = (ms: number): Promise<void> => new Promise((r) => setTimeout(r, ms));
async function waitFor(pred: () => boolean, timeoutMs = 8000, stepMs = 50): Promise<boolean> {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    if (pred()) return true;
    await sleep(stepMs);
  }
  return pred();
}

afterEach(() => {
  for (const d of dirs.splice(0)) {
    try {
      fs.rmSync(d, { recursive: true, force: true });
    } catch {
      /* ignore */
    }
  }
  vi.restoreAllMocks();
});

// ---------- load ----------

describe("ConfigMapConfigSource: load", () => {
  it("loads config from a mounted directory and reports its source name", async () => {
    const mount = tmpDir();
    fs.writeFileSync(path.join(mount, "config.json"), configJson(7));
    const src = new ConfigMapConfigSource(mount, "config.json");
    expect(src.sourceName()).toBe("CONFIGMAP");
    expect(await src.load()).toEqual({ component: { name: "x" }, version: 7 });
  });

  it("load() rejects on a missing key (initial load fails loudly, like FILE)", async () => {
    const mount = tmpDir();
    const src = new ConfigMapConfigSource(mount, "config.json");
    await expect(src.load()).rejects.toBeInstanceOf(EdgeCommonsError);
    await src.load().catch((e) => {
      expect((e as EdgeCommonsError).kind).toBe("Io");
      expect((e as EdgeCommonsError).message).toContain("config.json");
    });
  });

  it("load() rejects (Config) on malformed JSON", async () => {
    const mount = tmpDir();
    fs.writeFileSync(path.join(mount, "config.json"), "{ not json ]");
    const src = new ConfigMapConfigSource(mount, "config.json");
    await src.load().catch((e) => expect((e as EdgeCommonsError).kind).toBe("Config"));
    await expect(src.load()).rejects.toBeInstanceOf(EdgeCommonsError);
  });

  it("applies default mount dir + key (and warns it looks like a subPath mount)", () => {
    const warn = vi.spyOn(logger, "warn").mockImplementation(() => undefined);
    const src = new ConfigMapConfigSource(); // defaults: /etc/edgecommons + config.json
    expect(src.sourceName()).toBe("CONFIGMAP");
    expect(ConfigMapConfigSource.DEFAULT_MOUNT_DIR).toBe("/etc/edgecommons");
    expect(ConfigMapConfigSource.DEFAULT_KEY).toBe("config.json");
    // The default mount dir does not exist on a dev host -> no ..data -> subPath warning fires.
    expect(warn).toHaveBeenCalled();
    expect((warn.mock.calls[0][0] as string)).toContain("edgecommons");
  });
});

// ---------- dotfile filter (FR-CFG-4) ----------

describe("ConfigMapConfigSource: dotfile filter", () => {
  it("isProjectionArtifact identifies kubelet projection artifacts", () => {
    expect(isProjectionArtifact("..data")).toBe(true);
    expect(isProjectionArtifact("..2026_06_25_12_00_00.123456789")).toBe(true);
    expect(isProjectionArtifact("..data_tmp")).toBe(true);
    expect(isProjectionArtifact("config.json")).toBe(false);
  });

  it("rejects a key that is itself a projection artifact", () => {
    const mount = tmpDir();
    expect(() => new ConfigMapConfigSource(mount, "..data")).toThrow(EdgeCommonsError);
  });
});

// ---------- subPath warning (FR-CFG-3) ----------

describe("ConfigMapConfigSource: subPath warning", () => {
  it("warns when the mount has no '..data' symlink, but still constructs + loads", async () => {
    const mount = tmpDir();
    fs.writeFileSync(path.join(mount, "config.json"), configJson(1));
    const warn = vi.spyOn(logger, "warn").mockImplementation(() => undefined);
    const src = new ConfigMapConfigSource(mount, "config.json");
    expect(warn).toHaveBeenCalled();
    expect((warn.mock.calls[0][0] as string)).toContain("subPath");
    expect(await src.load()).toEqual({ component: { name: "x" }, version: 1 });
  });

  it("does not warn when '..data' is present (whole-volume mount)", () => {
    const mount = tmpDir();
    fs.mkdirSync(path.join(mount, "..data"));
    const warn = vi.spyOn(logger, "warn").mockImplementation(() => undefined);
    new ConfigMapConfigSource(mount, "config.json");
    expect(warn).not.toHaveBeenCalled();
  });
});

// ---------- reject-and-keep on reload (FR-CFG-5) ----------

describe("ConfigMapConfigSource: reject-and-keep on reload", () => {
  it("hot-reloads on a valid edit and keeps the previous config on a malformed edit", async () => {
    const mount = tmpDir();
    fs.mkdirSync(path.join(mount, "..data")); // whole-volume mount: no subPath warning noise
    const key = path.join(mount, "config.json");
    fs.writeFileSync(key, configJson(1));
    const warn = vi.spyOn(logger, "warn").mockImplementation(() => undefined);

    const src = new ConfigMapConfigSource(mount, "config.json");
    const updates: unknown[] = [];
    const watch = await src.watch((doc) => updates.push(doc));
    expect(watch).toBeDefined();
    await sleep(300); // let the directory watch arm

    // Valid edit -> onUpdate fires with the new doc.
    await fsp.writeFile(key, configJson(2));
    expect(await waitFor(() => updates.length >= 1)).toBe(true);
    expect(updates[updates.length - 1]).toEqual({ component: { name: "x" }, version: 2 });

    // Malformed edit -> NOT delivered (previous config stays in effect); a warning is logged.
    const before = updates.length;
    await fsp.writeFile(key, "{ broken ]");
    await sleep(500);
    expect(updates.length).toBe(before);
    expect(warn).toHaveBeenCalled();

    await watch!.close();
    // After close, further edits do not deliver.
    const afterClose = updates.length;
    await fsp.writeFile(key, configJson(3));
    await sleep(400);
    expect(updates.length).toBe(afterClose);
  });

  it("keeps the previous config when the key is empty (null JSON)", async () => {
    const mount = tmpDir();
    fs.mkdirSync(path.join(mount, "..data"));
    const key = path.join(mount, "config.json");
    fs.writeFileSync(key, configJson(1));
    vi.spyOn(logger, "warn").mockImplementation(() => undefined);

    const src = new ConfigMapConfigSource(mount, "config.json");
    const updates: unknown[] = [];
    const watch = await src.watch((doc) => updates.push(doc));
    await sleep(300);

    await fsp.writeFile(key, ""); // empty -> JSON.parse throws -> reject-and-keep
    await sleep(500);
    expect(updates.length).toBe(0);

    // A key that parses to literal `null` is also rejected-and-kept (no onUpdate(null)).
    await fsp.writeFile(key, "null");
    await sleep(500);
    expect(updates.length).toBe(0);
    await watch!.close();
  });

  it("keeps the previous config when the key vanishes mid-swap (read ENOENT)", async () => {
    // A reload that reads the key during a swap window (file briefly absent) must not crash: the read
    // error is logged and the previous config is kept (FR-CFG-5).
    const mount = tmpDir();
    fs.mkdirSync(path.join(mount, "..data"));
    const key = path.join(mount, "config.json");
    fs.writeFileSync(key, configJson(1));
    const warn = vi.spyOn(logger, "warn").mockImplementation(() => undefined);

    const src = new ConfigMapConfigSource(mount, "config.json");
    const updates: unknown[] = [];
    const watch = await src.watch((doc) => updates.push(doc));
    await sleep(300);

    fs.rmSync(key); // delete the key -> the watch fires -> reload reads -> ENOENT -> reject-and-keep
    await sleep(500);
    expect(updates.length).toBe(0);
    expect(warn).toHaveBeenCalled();
    await watch!.close();
  });
});

// ---------- directory-watch re-arm across swaps (FR-CFG-2) ----------

describe("ConfigMapConfigSource: directory-watch re-arm", () => {
  it("reloads repeatedly across successive edits (the watch is not one-shot)", async () => {
    const mount = tmpDir();
    fs.mkdirSync(path.join(mount, "..data"));
    const key = path.join(mount, "config.json");
    fs.writeFileSync(key, configJson(1));
    vi.spyOn(logger, "warn").mockImplementation(() => undefined);

    const src = new ConfigMapConfigSource(mount, "config.json");
    const updates: unknown[] = [];
    const watch = await src.watch((doc) => updates.push(doc));
    const lastVersion = (): number | undefined =>
      (updates[updates.length - 1] as { version: number } | undefined)?.version;
    await sleep(300);

    // Two successive edits (spaced so the OS watcher emits distinct events, not one coalesced one).
    // The watch must keep firing across both — i.e. it re-arms and is not a one-shot watch — so the
    // final observed config carries version 3. Asserting on the last value (not the event count)
    // is robust to the OS coalescing/duplicating watch events.
    await fsp.writeFile(key, configJson(2));
    expect(await waitFor(() => lastVersion() === 2)).toBe(true);
    await sleep(400);
    await fsp.writeFile(key, configJson(3));
    expect(await waitFor(() => lastVersion() === 3)).toBe(true);
    expect(updates.length).toBeGreaterThanOrEqual(2);
    await watch!.close();
  });

  it("re-arms when the mount directory does not exist yet, then reloads once it appears", async () => {
    // Watch a mount that does not exist: arming throws (ENOENT), the source backs off and retries
    // (re-arm). Once the directory + key appear, it begins delivering events. Deterministically
    // exercises the catch -> scheduleRearm -> arm loop on every OS.
    const parent = tmpDir();
    const mount = path.join(parent, "late-mount");
    vi.spyOn(logger, "warn").mockImplementation(() => undefined);

    const src = new ConfigMapConfigSource(mount, "config.json");
    const updates: unknown[] = [];
    const watch = await src.watch((doc) => updates.push(doc));

    await sleep(300); // let a few register-retry cycles elapse before the directory exists
    fs.mkdirSync(mount);
    await sleep(300); // let the watch arm on the newly-created directory
    fs.writeFileSync(path.join(mount, "config.json"), configJson(5));

    expect(await waitFor(() => updates.length >= 1, 10000)).toBe(true);
    expect((updates[updates.length - 1] as { version: number }).version).toBe(5);
    await watch!.close();
  });

  it("survives the kubelet atomic '..data' symlink swap (symlink-capable hosts only)", async () => {
    const mount = tmpDir();
    const firstData = path.join(mount, "..2026_a");
    fs.mkdirSync(firstData);
    fs.writeFileSync(path.join(firstData, "config.json"), configJson(1));

    // Build the faithful kubelet shape: config.json -> ..data/config.json, ..data -> ..2026_a.
    let symlinksWork = true;
    try {
      fs.symlinkSync("..2026_a", path.join(mount, "..data"), "dir");
      fs.symlinkSync(path.join("..data", "config.json"), path.join(mount, "config.json"), "file");
    } catch {
      symlinksWork = false;
    }
    if (!symlinksWork) {
      console.warn("symlinks not supported on this host; kubelet swap simulation skipped");
      return;
    }
    vi.spyOn(logger, "warn").mockImplementation(() => undefined);

    const src = new ConfigMapConfigSource(mount, "config.json");
    expect((await src.load()) as { version: number }).toEqual({ component: { name: "x" }, version: 1 });

    const updates: unknown[] = [];
    const watch = await src.watch((doc) => updates.push(doc));
    await sleep(400); // let the directory watch arm before the swap

    // Kubelet swap: new timestamped dir, stage ..data_tmp -> it, atomic rename onto ..data.
    const secondData = path.join(mount, "..2026_b");
    fs.mkdirSync(secondData);
    fs.writeFileSync(path.join(secondData, "config.json"), configJson(2));
    fs.symlinkSync("..2026_b", path.join(mount, "..data_tmp"), "dir");
    fs.renameSync(path.join(mount, "..data_tmp"), path.join(mount, "..data"));

    expect(await waitFor(() => updates.length >= 1, 10000)).toBe(true);
    expect((updates[updates.length - 1] as { version: number }).version).toBe(2);
    await watch!.close();
  });
});

// ---------- dispatch ----------

describe("buildConfigSource: CONFIGMAP", () => {
  it("dispatches CONFIGMAP without extra deps", () => {
    vi.spyOn(logger, "warn").mockImplementation(() => undefined);
    const src = buildConfigSource(
      { kind: "CONFIGMAP", mountDir: undefined, key: undefined },
      { thingName: "T", componentName: "C" },
    );
    expect(src).toBeInstanceOf(ConfigMapConfigSource);
    expect(src.sourceName()).toBe("CONFIGMAP");
  });
});
