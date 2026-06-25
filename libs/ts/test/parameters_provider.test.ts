/**
 * Parameter provider/source + config branch top-ups: the error and default-value paths the main
 * parameters.test.ts leaves uncovered. No AWS — env/mountedDir sources and the in-process service.
 */
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "fs";
import { tmpdir } from "os";
import { join } from "path";

import { afterEach, describe, expect, it, vi } from "vitest";

/**
 * Controllable fs mock: by default it delegates to the real fs (so every other test is unaffected);
 * a test can set `fsFault.readFileCode` to make `readFileSync` throw a chosen errno, which is the
 * only portable way to hit the non-ENOENT read-error branches in the source on Windows.
 */
const fsFault: { readFileCode?: string } = {};
vi.mock("fs", async (importOriginal) => {
  const actual = await importOriginal<typeof import("fs")>();
  return {
    ...actual,
    readFileSync: (...args: Parameters<typeof actual.readFileSync>) => {
      if (fsFault.readFileCode) {
        const e = new Error(`injected ${fsFault.readFileCode}`) as NodeJS.ErrnoException;
        e.code = fsFault.readFileCode;
        throw e;
      }
      return actual.readFileSync(...args);
    },
  };
});

import { openFromConfig } from "../src/parameters/config";
import { ParameterError } from "../src/parameters/errors";
import { DefaultParameterService } from "../src/parameters/service";
import { EnvSource, MountedDirSource } from "../src/parameters/source";

const setEnv: string[] = [];
function env(name: string, value: string): void {
  process.env[name] = value;
  setEnv.push(name);
}
afterEach(() => {
  for (const n of setEnv.splice(0)) delete process.env[n];
  fsFault.readFileCode = undefined;
});

describe("EnvSource fetchByPath", () => {
  it("ignores env vars outside the prefix and surfaces only matching names under the path", async () => {
    env("GGPP_ENV_MYAPP_A", "1");
    env("GGPP_ENV_MYAPP_B", "2");
    env("GGPP_ENV_OTHER_C", "3");
    env("UNRELATED_VAR_XYZ", "nope"); // outside the prefix => fromEnv returns undefined branch
    const source = new EnvSource("GGPP_ENV_");
    const items = await source.fetchByPath("/myapp", false);
    const names = items.map(([n]) => n).sort();
    expect(names).toEqual(["/myapp/a", "/myapp/b"]);
    // None of the matched values are flagged secure (env is plaintext).
    expect(items.every(([, v]) => v.secure === false)).toBe(true);
  });

  it("default-prefix EnvSource (no explicit prefix via config) reads GG_PARAM_*", async () => {
    env("GG_PARAM_DEFPFX_KEY", "fromdefault");
    const cfg = {
      source: { type: "env" }, // prefix omitted => default "GG_PARAM_" branch in config.buildSource
      bootstrapOnStart: false,
      refreshIntervalSecs: 0,
      sync: { names: ["/defpfx/key"] },
    };
    const s = await openFromConfig(cfg);
    try {
      await s.refresh();
      expect(s.get("/defpfx/key")).toBe("fromdefault");
    } finally {
      s.close();
    }
  });
});

describe("MountedDirSource error paths", () => {
  it("fetch surfaces a non-ENOENT read error as a ParameterError (not undefined)", async () => {
    const dir = mkdtempSync(join(tmpdir(), "ggpp-fetcherr-"));
    writeFileSync(join(dir, "k"), "v");
    const source = new MountedDirSource(dir, []);
    // statSync(file).isDirectory() is false, then readFileSync throws a non-ENOENT errno => the
    // catch must re-wrap as ParameterError instead of swallowing it as "missing".
    fsFault.readFileCode = "EIO";
    await expect(source.fetch("/k")).rejects.toThrow(ParameterError);
  });

  it("walk surfaces a non-ENOENT read error inside fetchByPath as a ParameterError", async () => {
    const dir = mkdtempSync(join(tmpdir(), "ggpp-walkerr-"));
    writeFileSync(join(dir, "k"), "v");
    const source = new MountedDirSource(dir, []);
    // readdir lists the entry; readFileSync then fails with a non-ENOENT errno => walk re-wraps it.
    fsFault.readFileCode = "EACCES";
    await expect(source.fetchByPath("/", true)).rejects.toThrow(ParameterError);
  });
});

describe("DefaultParameterService getBool false-words", () => {
  it("parses 0 and no (not just off) as false", async () => {
    env("GGPP_BOOL_ZERO", "0");
    env("GGPP_BOOL_NO", "NO");
    const source = new EnvSource("GGPP_BOOL_");
    const s = DefaultParameterService.withMemoryCache(source, ["/zero", "/no"], []);
    await s.refresh();
    expect(s.getBool("/zero")).toBe(false);
    expect(s.getBool("/no")).toBe(false);
  });
});

describe("background refresh error path", () => {
  it("a failing background refresh is swallowed (logged), not thrown", async () => {
    vi.useFakeTimers();
    try {
      // A source that errors on every fetch; seed nothing so refresh rejects, exercising the
      // setInterval(...).catch branch in withRefresh.
      const failing = {
        fetch: async () => {
          throw new ParameterError("boom");
        },
        fetchByPath: async () => {
          throw new ParameterError("boom");
        },
        sourceId: () => "failing",
      };
      const s = DefaultParameterService.withMemoryCache(failing, ["/x"], []).withRefresh(1);
      // Advancing the timer triggers the background refresh; the rejection must be caught internally.
      await vi.advanceTimersByTimeAsync(1100);
      expect(s.stats().refreshFailures).toBeGreaterThanOrEqual(1);
      s.close();
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("openFromConfig persistent default cache path", () => {
  it("cache.persist=true with no cache.path uses the default 'param-cache' path", async () => {
    const dir = mkdtempSync(join(tmpdir(), "ggpp-defcache-"));
    mkdirSync(join(dir, "src"), { recursive: true });
    writeFileSync(join(dir, "src", "k"), "v");
    const prevCwd = process.cwd();
    process.chdir(dir); // default "param-cache" is created relative to cwd
    try {
      const cfg = {
        source: { type: "mountedDir", root: dir },
        cache: { persist: true }, // no path => default "param-cache" branch in config.openFromConfig
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
    } finally {
      process.chdir(prevCwd);
      try {
        rmSync(join(dir, "param-cache"), { force: true });
        rmSync(join(dir, "param-cache.key"), { force: true });
      } catch {
        /* best effort */
      }
    }
  });
});
