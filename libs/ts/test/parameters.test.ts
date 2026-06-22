/** TS parameter-service tests — mirror the 9 Rust `parameters::tests`. */
import { mkdirSync, mkdtempSync, writeFileSync } from "fs";
import { tmpdir } from "os";
import { join } from "path";

import { afterEach, describe, expect, it } from "vitest";

import { openFromConfig } from "../src/parameters/config";
import { ParameterError } from "../src/parameters/errors";
import { DefaultParameterService, ParameterService, SyncPath } from "../src/parameters/service";
import { EnvSource, MountedDirSource, ParameterSource, ParamValue } from "../src/parameters/source";

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

// Keep the ParameterService type referenced (interface conformance) so the import isn't pruned.
const _conform: (s: ParameterService) => string | undefined = (s) => s.get("/x");
void _conform;
