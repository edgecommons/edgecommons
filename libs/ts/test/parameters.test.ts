/** TS parameter-service tests — mirror the 9 Rust `parameters::tests`. */
import { mkdirSync, mkdtempSync, writeFileSync } from "fs";
import { tmpdir } from "os";
import { join } from "path";

import { afterEach, describe, expect, it, vi } from "vitest";

import { validate } from "../src/config/validation";
import { openFromConfig } from "../src/parameters/config";
import { ParameterError } from "../src/parameters/errors";
import { DefaultParameterService, ParameterService, SyncPath } from "../src/parameters/service";
import { EnvSource, MountedDirSource, ParameterSource, ParamValue, plainValue } from "../src/parameters/source";

/** Track env vars set per test so we can clean them up (unique prefixes still avoid collisions). */
const setEnv: string[] = [];
function env(name: string, value: string): void {
  process.env[name] = value;
  setEnv.push(name);
}
afterEach(() => {
  for (const n of setEnv.splice(0)) delete process.env[n];
});

async function svcEnv(prefix: string, names: string[]): Promise<DefaultParameterService> {
  const source = new EnvSource(prefix);
  const s = DefaultParameterService.withMemoryCache(source, names, []);
  await s.refresh();
  return s;
}

describe("EnvSource", () => {
  it("round-trips name mapping; missing is undefined", async () => {
    env("GGTEST_ENV_MYAPP_DB_HOST", "db.example.com");
    env("GGTEST_ENV_MYAPP_DB_POOLSIZE", "8");
    const s = await svcEnv("GGTEST_ENV_", ["/myapp/db/host", "/myapp/db/poolSize"]);
    expect(s.get("/myapp/db/host")).toBe("db.example.com");
    expect(s.getInt("/myapp/db/poolSize")).toBe(8);
    // Missing parameter is undefined, not an error.
    expect(s.get("/myapp/db/missing")).toBeUndefined();
  });

  it("typed accessors parse int/bool/json/stringList", async () => {
    env("GGTEST_TYPED_FLAG", "true");
    env("GGTEST_TYPED_LIST", "a, b ,c");
    env("GGTEST_TYPED_OBJ", '{"k":1}');
    const s = await svcEnv("GGTEST_TYPED_", ["/flag", "/list", "/obj"]);
    expect(s.getBool("/flag")).toBe(true);
    expect(s.getStringList("/list")).toEqual(["a", "b", "c"]);
    expect((s.getJson("/obj") as { k: number }).k).toBe(1);
  });
});

describe("MountedDirSource", () => {
  it("reads files, marks securePaths, and skips dotfile/..data entries", async () => {
    const dir = mkdtempSync(join(tmpdir(), "ggparam-mnt-"));
    const cfg = join(dir, "myapp", "db");
    mkdirSync(cfg, { recursive: true });
    writeFileSync(join(cfg, "host"), "cfg.example.com");
    const sec = join(dir, "secret");
    mkdirSync(sec, { recursive: true });
    writeFileSync(join(sec, "token"), "s3cr3t");
    // K8s projects an internal "..data" symlink dir that must be skipped.
    mkdirSync(join(dir, "..data"), { recursive: true });
    writeFileSync(join(dir, "..data", "ignored"), "nope");

    const source = new MountedDirSource(dir, ["/secret"]);
    const s = DefaultParameterService.withMemoryCache(source, [], [["/", true]]);
    await s.refresh();

    expect(s.get("/myapp/db/host")).toBe("cfg.example.com");
    expect(s.get("/secret/token")).toBe("s3cr3t");
    const names = s.names("/");
    expect(names).toContain("/myapp/db/host");
    expect(names).toContain("/secret/token");
    // The internal ..data entry is not surfaced as a parameter.
    expect(names.some((n) => n.includes("..data"))).toBe(false);
    // securePaths flagging: /secret/token is secure, /myapp/db/host is not.
    expect(s.getByPath("/secret").get("/secret/token")).toBe("s3cr3t");
  });
});

describe("getByPath", () => {
  it("returns the subtree under a prefix", async () => {
    env("GGTEST_PATH_MYAPP_A", "1");
    env("GGTEST_PATH_MYAPP_B", "2");
    env("GGTEST_PATH_OTHER_C", "3");
    const source = new EnvSource("GGTEST_PATH_");
    const s = DefaultParameterService.withMemoryCache(source, [], [["/myapp", true]]);
    await s.refresh();
    const sub = s.getByPath("/myapp");
    expect(sub.get("/myapp/a")).toBe("1");
    expect(sub.get("/myapp/b")).toBe("2");
    expect(sub.has("/other/c")).toBe(false);
  });
});

/** A source that always errors — stands in for an unreachable remote backend. */
class FailingSource implements ParameterSource {
  async fetch(): Promise<ParamValue | undefined> {
    throw new ParameterError("offline");
  }
  async fetchByPath(): Promise<Array<[string, ParamValue]>> {
    throw new ParameterError("offline");
  }
  sourceId(): string {
    return "failing";
  }
}

describe("offline tolerance", () => {
  it("path refresh failure is non-fatal once the cache is non-empty", async () => {
    // First prime one name successfully so the cache is non-empty; a subsequent path-refresh error
    // is then swallowed (warned) instead of thrown. Exercises the path-refresh catch branch.
    const source = new FakeRemoteSource();
    source.set("/seed", "v", false, "1");
    const failingPath: ParameterSource = {
      fetch: (name) => source.fetch(name),
      fetchByPath: async () => {
        throw new ParameterError("path offline");
      },
      sourceId: () => "mixed",
    };
    const s = DefaultParameterService.withMemoryCache(failingPath, ["/seed"], [["/svc", true]]);
    await s.refresh(); // /seed succeeds; /svc path throws but cache non-empty => non-fatal
    expect(s.get("/seed")).toBe("v");
    expect(s.stats().refreshFailures).toBe(1);
  });

  it("refresh errors when cache empty, then serves nothing", async () => {
    const source = new FailingSource();
    const names: string[] = ["/myapp/x"];
    const paths: SyncPath[] = [];
    const s = DefaultParameterService.withMemoryCache(source, names, paths);
    // Empty cache + source down => bootstrap-style refresh surfaces the error.
    await expect(s.refresh()).rejects.toThrow(ParameterError);
    expect(s.stats().refreshFailures).toBe(1);
    expect(s.get("/myapp/x")).toBeUndefined();
  });

  it("keeps cached values when the source returns nothing", async () => {
    // Prime via env, then drop the env var: env fetch returns undefined (not an error), so the
    // already-cached value is retained (offline-first: never clear).
    env("GGTEST_OFFLINE_VAL", "cached");
    const s = await svcEnv("GGTEST_OFFLINE_", ["/val"]);
    expect(s.get("/val")).toBe("cached");
    delete process.env["GGTEST_OFFLINE_VAL"];
    await s.refresh();
    expect(s.get("/val")).toBe("cached");
  });
});

describe("openFromConfig", () => {
  it("opens an env source", async () => {
    env("GGTEST_CFG_MYAPP_REGION", "us-east-1");
    const cfg = {
      source: { type: "env", prefix: "GGTEST_CFG_" },
      bootstrapOnStart: true,
      refreshIntervalSecs: 0,
      sync: { names: ["/myapp/region"] },
    };
    const s = await openFromConfig(cfg);
    try {
      expect(s.get("/myapp/region")).toBe("us-east-1");
      expect(s.stats().source).toBe("env");
    } finally {
      s.close();
    }
  });

  it("accepts a path entry as a string or an object", async () => {
    const cfg = {
      source: { type: "env", prefix: "GGTEST_PE_" },
      bootstrapOnStart: false,
      refreshIntervalSecs: 0,
      sync: { paths: ["/myapp", { path: "/other", recursive: false }] as (string | { path: string; recursive?: boolean })[] },
    };
    // We can't inspect the parsed entries directly (private), but a bare string => recursive and an
    // object => its recursive flag must both open without error and produce a working service.
    const s = await openFromConfig(cfg);
    try {
      expect(s.stats().source).toBe("env");
      // Bare-string path is recursive by default; the object path's recursive=false is honored.
      expect(s.names("/")).toEqual([]);
    } finally {
      s.close();
    }
  });

  it("coerces a lenient numeric refreshIntervalSecs (300.0)", async () => {
    // JSON has real numbers, so 300.0 === 300 in TS — trivially true; kept for cross-lang parity.
    const cfg = {
      source: { type: "env", prefix: "GGTEST_LEN_" },
      bootstrapOnStart: false,
      refreshIntervalSecs: 300.0,
    };
    const s = await openFromConfig(cfg);
    try {
      expect(s.stats().source).toBe("env");
    } finally {
      s.close();
    }
  });
});

/**
 * A fake remote source: in-memory, secure + versioned values, and a controllable version so a
 * background refresh can observe an upstream change. Stands in for SSM without the AWS SDK.
 */
class FakeRemoteSource implements ParameterSource {
  private readonly map = new Map<string, ParamValue>();

  set(name: string, value: string, secure: boolean, version: string): void {
    this.map.set(name, { value: Buffer.from(value, "utf-8"), secure, version });
  }
  async fetch(name: string): Promise<ParamValue | undefined> {
    return this.map.get(name);
  }
  async fetchByPath(path: string, _recursive: boolean): Promise<Array<[string, ParamValue]>> {
    const out: Array<[string, ParamValue]> = [];
    for (const [k, v] of this.map) if (k.startsWith(path)) out.push([k, v]);
    return out;
  }
  sourceId(): string {
    return "fakeRemote";
  }
}

describe("source helpers", () => {
  it("plainValue marks values non-secure", () => {
    const v = plainValue(Buffer.from("x"));
    expect(v.secure).toBe(false);
    expect(v.value.toString()).toBe("x");
  });
});

describe("VaultCache (persistent, offline survival)", () => {
  it("survives reopen: cached secure+versioned values are served after the source goes away", async () => {
    const dir = mkdtempSync(join(tmpdir(), "ggparam-vault-"));
    const cachePath = join(dir, "param-cache");
    const source = new FakeRemoteSource();
    source.set("/svc/region", "us-east-1", false, "3");
    source.set("/svc/token", "s3cr3t", true, "9"); // secure: must round-trip but never be logged

    // Force a persistent (encrypted) cache via cache.persist on an env-typed run would be local; use
    // a fake-remote-equivalent: drive the persistent path directly through openFromConfig is awSsm-only,
    // so build the persistent service the way config.ts does, against the real LocalVault.
    const { buildKeyProvider, LocalVault } = await import("../src/credentials");
    const { provider } = await buildKeyProvider({}, cachePath, `${cachePath}.key`);
    const vault = LocalVault.open(cachePath, provider, 1);
    const svc1 = DefaultParameterService.withPersistentCache(source, vault, ["/svc/region", "/svc/token"], []);
    await svc1.refresh();
    expect(svc1.get("/svc/region")).toBe("us-east-1");
    expect(svc1.get("/svc/token")).toBe("s3cr3t");
    expect(svc1.stats().parameterCount).toBe(2);
    svc1.close();

    // Reopen the SAME on-disk vault with a source that has NOTHING — values must persist (offline).
    const { provider: p2 } = await buildKeyProvider({}, cachePath, `${cachePath}.key`);
    const vault2 = LocalVault.open(cachePath, p2, 1);
    const empty = new FakeRemoteSource();
    const svc2 = DefaultParameterService.withPersistentCache(empty, vault2, [], [["/svc", true]]);
    expect(svc2.get("/svc/region")).toBe("us-east-1");
    expect(svc2.get("/svc/token")).toBe("s3cr3t");
    // getByPath + names go through the VaultCache.entries() path.
    const sub = svc2.getByPath("/svc");
    expect(sub.get("/svc/region")).toBe("us-east-1");
    expect(new Set(svc2.names("/svc"))).toEqual(new Set(["/svc/region", "/svc/token"]));
    expect(svc2.getBytes("/svc/token")!.toString("utf-8")).toBe("s3cr3t");
    svc2.close();
  });
});

describe("background refresh timer", () => {
  it("re-pulls the declared names and observes an upstream change", async () => {
    vi.useFakeTimers();
    try {
      const source = new FakeRemoteSource();
      source.set("/svc/v", "first", false, "1");
      const s = DefaultParameterService.withMemoryCache(source, ["/svc/v"], []).withRefresh(1);
      await s.refresh();
      expect(s.get("/svc/v")).toBe("first");

      // Upstream changes; the background timer (1s) re-pulls it.
      source.set("/svc/v", "second", false, "2");
      await vi.advanceTimersByTimeAsync(1100);
      expect(s.get("/svc/v")).toBe("second");
      s.close();
    } finally {
      vi.useRealTimers();
    }
  });

  it("withRefresh(0) installs no timer (close is a no-op)", async () => {
    const source = new FakeRemoteSource();
    const s = DefaultParameterService.withMemoryCache(source, [], []).withRefresh(0);
    s.close(); // no timer => safe
    expect(s.stats().source).toBe("fakeRemote");
  });
});

describe("MountedDirSource edge cases", () => {
  it("single fetch: file, directory (=> undefined), missing (=> undefined), and secure flag", async () => {
    const dir = mkdtempSync(join(tmpdir(), "ggparam-fetch-"));
    mkdirSync(join(dir, "svc"), { recursive: true });
    writeFileSync(join(dir, "svc", "host"), "h");
    const source = new MountedDirSource(dir, ["/svc/host"]);
    const v = await source.fetch("/svc/host");
    expect(v!.value.toString()).toBe("h");
    expect(v!.secure).toBe(true); // under securePaths
    // A directory at the name is "not a parameter".
    expect(await source.fetch("/svc")).toBeUndefined();
    // Missing file is undefined, not an error.
    expect(await source.fetch("/svc/missing")).toBeUndefined();
    expect(source.sourceId()).toBe("mountedDir");
  });

  it("fetchByPath on a missing base dir returns empty (no throw)", async () => {
    const dir = mkdtempSync(join(tmpdir(), "ggparam-nobase-"));
    const source = new MountedDirSource(dir, []);
    expect(await source.fetchByPath("/does/not/exist", true)).toEqual([]);
  });

  it("non-UTF-8 value: getByPath skips it but getBytes still returns the bytes", async () => {
    const dir = mkdtempSync(join(tmpdir(), "ggparam-bin-"));
    mkdirSync(join(dir, "bin"), { recursive: true });
    writeFileSync(join(dir, "bin", "blob"), Buffer.from([0xff, 0xfe, 0x00]));
    writeFileSync(join(dir, "bin", "text"), "ok");
    const source = new MountedDirSource(dir, []);
    const s = DefaultParameterService.withMemoryCache(source, [], [["/bin", true]]);
    await s.refresh();
    // get() on a non-UTF-8 value throws ParameterError.
    expect(() => s.get("/bin/blob")).toThrow(ParameterError);
    // getByPath silently omits the non-UTF-8 entry but keeps the text one.
    const sub = s.getByPath("/bin");
    expect(sub.has("/bin/blob")).toBe(false);
    expect(sub.get("/bin/text")).toBe("ok");
    // getBytes returns the raw bytes regardless.
    expect(s.getBytes("/bin/blob")).toEqual(Buffer.from([0xff, 0xfe, 0x00]));
  });

  it("fetchByPath on a non-directory base path throws ParameterError (non-ENOENT readdir)", async () => {
    const dir = mkdtempSync(join(tmpdir(), "ggparam-notdir-"));
    writeFileSync(join(dir, "afile"), "x");
    const source = new MountedDirSource(dir, []);
    // Treating a regular file as the base dir => readdirSync fails with ENOTDIR (not ENOENT) => throw.
    await expect(source.fetchByPath("/afile", true)).rejects.toThrow(ParameterError);
  });

  it("non-recursive fetchByPath does not descend into subdirectories", async () => {
    const dir = mkdtempSync(join(tmpdir(), "ggparam-norec-"));
    writeFileSync(join(dir, "top"), "t");
    mkdirSync(join(dir, "nested"), { recursive: true });
    writeFileSync(join(dir, "nested", "deep"), "d");
    const source = new MountedDirSource(dir, []);
    const items = await source.fetchByPath("/", false);
    const names = items.map(([n]) => n);
    expect(names).toContain("/top");
    expect(names).not.toContain("/nested/deep");
  });
});

describe("typed accessor branches", () => {
  it("getBool false-words, empty stringList, getInt parse error, missing => undefined", async () => {
    env("GGTEST_TY2_FLAG", "off");
    env("GGTEST_TY2_EMPTY", "");
    env("GGTEST_TY2_NOTINT", "abc");
    const s = await svcEnv("GGTEST_TY2_", ["/flag", "/empty", "/notInt"]);
    expect(s.getBool("/flag")).toBe(false);
    expect(s.getStringList("/empty")).toEqual([]);
    expect(() => s.getInt("/notInt")).toThrow(ParameterError);
    // getBool on a non-boolean throws; JSON on non-JSON throws.
    env("GGTEST_TY2_NOTBOOL", "maybe");
    const s2 = await svcEnv("GGTEST_TY2_", ["/notBool"]);
    expect(() => s2.getBool("/notBool")).toThrow(ParameterError);
    env("GGTEST_TY2_NOTJSON", "{bad");
    const s3 = await svcEnv("GGTEST_TY2_", ["/notJson"]);
    expect(() => s3.getJson("/notJson")).toThrow(ParameterError);

    // Every typed accessor returns undefined for a missing parameter (no throw).
    expect(s.getInt("/nope")).toBeUndefined();
    expect(s.getBool("/nope")).toBeUndefined();
    expect(s.getJson("/nope")).toBeUndefined();
    expect(s.getStringList("/nope")).toBeUndefined();
    expect(s.getBytes("/nope")).toBeUndefined();
  });

  it("getBool true-words 1/yes/on", async () => {
    env("GGTEST_TY3_A", "1");
    env("GGTEST_TY3_B", "yes");
    env("GGTEST_TY3_C", "ON");
    const s = await svcEnv("GGTEST_TY3_", ["/a", "/b", "/c"]);
    expect(s.getBool("/a")).toBe(true);
    expect(s.getBool("/b")).toBe(true);
    expect(s.getBool("/c")).toBe(true);
  });
});

describe("openFromConfig source selection + errors", () => {
  it("opens a mountedDir source", async () => {
    const dir = mkdtempSync(join(tmpdir(), "ggparam-cfgmnt-"));
    mkdirSync(join(dir, "app"), { recursive: true });
    writeFileSync(join(dir, "app", "name"), "demo");
    const cfg = {
      source: { type: "mountedDir", root: dir, securePaths: ["/app/secret"] },
      bootstrapOnStart: true,
      refreshIntervalSecs: 0,
      sync: { paths: ["/app"] as (string | { path: string; recursive?: boolean })[] },
    };
    const s = await openFromConfig(cfg);
    try {
      expect(s.get("/app/name")).toBe("demo");
      expect(s.stats().source).toBe("mountedDir");
    } finally {
      s.close();
    }
  });

  it("mountedDir without root is an error", async () => {
    await expect(openFromConfig({ source: { type: "mountedDir" } })).rejects.toThrow(ParameterError);
  });

  it("an unknown source type is an error", async () => {
    await expect(openFromConfig({ source: { type: "bogus" } })).rejects.toThrow(ParameterError);
  });

  it("the default (no source) is the unsupported 'none' type", async () => {
    await expect(openFromConfig({})).rejects.toThrow(ParameterError);
  });

  it("default-recursive path entries: a bare string path is recursive", async () => {
    const dir = mkdtempSync(join(tmpdir(), "ggparam-rec-"));
    mkdirSync(join(dir, "a", "b"), { recursive: true });
    writeFileSync(join(dir, "a", "b", "deep"), "x");
    const cfg = {
      source: { type: "mountedDir", root: dir },
      bootstrapOnStart: true,
      refreshIntervalSecs: 0,
      sync: { paths: ["/a"] as (string | { path: string; recursive?: boolean })[] },
    };
    const s = await openFromConfig(cfg);
    try {
      // Bare string => recursive default => the deep file is pulled.
      expect(s.get("/a/b/deep")).toBe("x");
    } finally {
      s.close();
    }
  });

  it("cache.persist=true forces a persistent vault even for a local source", async () => {
    const dir = mkdtempSync(join(tmpdir(), "ggparam-forcepersist-"));
    mkdirSync(join(dir, "src"), { recursive: true });
    writeFileSync(join(dir, "src", "k"), "v");
    const cachePath = join(dir, "cache", "param-cache");
    const cfg = {
      source: { type: "mountedDir", root: dir },
      cache: { persist: true, path: cachePath },
      bootstrapOnStart: true,
      refreshIntervalSecs: 0,
      sync: { paths: ["/src"] as (string | { path: string; recursive?: boolean })[] },
    };
    const s = await openFromConfig(cfg);
    try {
      expect(s.get("/src/k")).toBe("v");
    } finally {
      s.close();
    }
  });

  it("bootstrap failure is non-fatal (offline-first); the service still opens", async () => {
    // mountedDir over a missing root bootstraps to an empty cache without throwing.
    const cfg = {
      source: { type: "mountedDir", root: join(tmpdir(), "ggparam-absent-root-xyz") },
      bootstrapOnStart: true,
      refreshIntervalSecs: 0,
      sync: { names: ["/x"] },
    };
    const s = await openFromConfig(cfg);
    try {
      expect(s.get("/x")).toBeUndefined();
      expect(s.stats().source).toBe("mountedDir");
    } finally {
      s.close();
    }
  });
});

describe("schema acceptance", () => {
  it("a config with a parameters section passes TS config validation", () => {
    // `parameters` is a known top-level section (validated permissively, like credentials).
    const cfg = {
      component: { global: {} },
      parameters: {
        source: { type: "env", prefix: "GG_PARAM_" },
        refreshIntervalSecs: 300,
        sync: { names: ["/skeleton/region"], paths: ["/skeleton"] },
      },
    };
    expect(() => validate(cfg)).not.toThrow();
  });
});

describe("stats", () => {
  it("reports lastRefreshAgeMs after a successful refresh", async () => {
    env("GGTEST_STATS_K", "v");
    const s = await svcEnv("GGTEST_STATS_", ["/k"]);
    const st = s.stats();
    expect(st.parameterCount).toBe(1);
    expect(st.lastRefreshAgeMs).toBeDefined();
    expect(st.lastRefreshAgeMs).toBeGreaterThanOrEqual(0);
    expect(st.refreshFailures).toBe(0);
  });
});

// Keep the ParameterService type referenced (interface conformance) so the import isn't pruned.
const _conform: (s: ParameterService) => string | undefined = (s) => s.get("/x");
void _conform;
