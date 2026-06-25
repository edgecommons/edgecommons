/** Sync engine unit tests: bootstrap, periodic refresh, offline-tolerant keep-cached-on-failure,
 * rotation/no-churn change detection, centralId override, and stats. The central source is a
 * fake {@link CentralVaultSource} (no AWS), and the vault is a real on-disk {@link LocalVault}. */
import { mkdtempSync } from "fs";
import { tmpdir } from "os";
import { join } from "path";

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { CentralSecret, CentralVaultSource } from "../src/credentials/central";
import { FileKeyProvider } from "../src/credentials/keyprovider";
import { SyncEngine } from "../src/credentials/sync";
import { LocalVault } from "../src/credentials/vault";

function newVault(): LocalVault {
  const dir = mkdtempSync(join(tmpdir(), "ggvault-sync-"));
  return LocalVault.open(join(dir, "vault"), new FileKeyProvider(Buffer.alloc(32, 11)), 2);
}

/** A scriptable in-memory central source. `responses` maps central id → a queue of outcomes; each
 * `fetch` consumes the next entry (or repeats the last). An entry may be a {@link CentralSecret},
 * `undefined` (not found), or an `Error` (thrown). */
class FakeSource implements CentralVaultSource {
  fetch = vi.fn(async (name: string): Promise<CentralSecret | undefined> => {
    const q = this.responses.get(name);
    if (!q || q.length === 0) return undefined;
    const next = q.length > 1 ? q.shift()! : q[0];
    if (next instanceof Error) throw next;
    return next;
  });
  readonly responses = new Map<string, (CentralSecret | undefined | Error)[]>();
  set(name: string, ...outcomes: (CentralSecret | undefined | Error)[]): void {
    this.responses.set(name, outcomes);
  }
}

function cs(value: string, centralVersionId: string, labels: Record<string, string> = {}): CentralSecret {
  return { bytes: Buffer.from(value), centralVersionId, labels };
}

describe("sync engine bootstrap", () => {
  it("seeds the vault from central on bootstrap", async () => {
    const vault = newVault();
    const src = new FakeSource();
    src.set("db/pw", cs("v1", "cv-1"));
    const e = await SyncEngine.start(vault, src, "", [["db/pw", undefined]], 0, true);

    expect(vault.get("db/pw")!.asString()).toBe("v1");
    expect(src.fetch).toHaveBeenCalledWith("db/pw");
    expect(e.stats().rotations).toBe(1);
    expect(e.stats().failures).toBe(0);
    expect(e.stats().lastSuccessMs).toBeGreaterThan(0);
    e.close();
  });

  it("does not seed when bootstrap is false", async () => {
    const vault = newVault();
    const src = new FakeSource();
    src.set("db/pw", cs("v1", "cv-1"));
    const e = await SyncEngine.start(vault, src, "", [["db/pw", undefined]], 0, false);

    expect(vault.get("db/pw")).toBeUndefined();
    expect(src.fetch).not.toHaveBeenCalled();
    e.close();
  });

  it("applies the namespace to the local key but uses it as the default central id", async () => {
    const vault = newVault();
    const src = new FakeSource();
    src.set("thing/Comp/db/pw", cs("v1", "cv-1"));
    const e = await SyncEngine.start(vault, src, "thing/Comp", [["db/pw", undefined]], 0, true);

    expect(src.fetch).toHaveBeenCalledWith("thing/Comp/db/pw");
    expect(vault.get("thing/Comp/db/pw")!.asString()).toBe("v1");
    e.close();
  });

  it("uses the centralId override (shared/fleet id) instead of the namespaced path", async () => {
    const vault = newVault();
    const src = new FakeSource();
    src.set("fleet/shared", cs("shared", "cv-1"));
    const e = await SyncEngine.start(vault, src, "thing/Comp", [["db/pw", "fleet/shared"]], 0, true);

    expect(src.fetch).toHaveBeenCalledWith("fleet/shared");
    // stored under the local namespaced key, not the override
    expect(vault.get("thing/Comp/db/pw")!.asString()).toBe("shared");
    e.close();
  });

  it("skips a secret whose central source returns undefined (not found)", async () => {
    const vault = newVault();
    const src = new FakeSource();
    src.set("db/pw", undefined);
    const e = await SyncEngine.start(vault, src, "", [["db/pw", undefined]], 0, true);

    expect(vault.get("db/pw")).toBeUndefined();
    expect(e.stats().rotations).toBe(0);
    // a successful (200-ish) fetch that returned no secret still counts as a successful pass
    expect(e.stats().lastSuccessMs).toBeGreaterThan(0);
    expect(e.stats().failures).toBe(0);
    e.close();
  });
});

describe("sync engine rotation + change detection", () => {
  it("rotates the cached value when the central version changes, but not when unchanged", async () => {
    const vault = newVault();
    const src = new FakeSource();
    src.set("db/pw", cs("v1", "cv-1"));
    const e = await SyncEngine.start(vault, src, "", [["db/pw", undefined]], 0, true);
    expect(vault.get("db/pw")!.asString()).toBe("v1");
    expect(e.stats().rotations).toBe(1);

    // same centralVersionId → no churn
    src.set("db/pw", cs("v1-again", "cv-1"));
    await e.syncNow();
    expect(vault.get("db/pw")!.asString()).toBe("v1");
    expect(vault.versions("db/pw")).toHaveLength(1);
    expect(e.stats().rotations).toBe(1);

    // new centralVersionId → rotate, prior version retained
    src.set("db/pw", cs("v2", "cv-2"));
    await e.syncNow();
    expect(vault.get("db/pw")!.asString()).toBe("v2");
    expect(vault.versions("db/pw")).toHaveLength(2);
    expect(vault.latestCentralVersionId("db/pw")).toBe("cv-2");
    expect(e.stats().rotations).toBe(2);
    e.close();
  });

  it("carries central labels into the stored secret", async () => {
    const vault = newVault();
    const src = new FakeSource();
    src.set("db/pw", cs("v1", "cv-1", { env: "prod" }));
    const e = await SyncEngine.start(vault, src, "", [["db/pw", undefined]], 0, true);
    expect(vault.list("")[0].labels).toEqual({ env: "prod" });
    e.close();
  });
});

describe("sync engine offline tolerance", () => {
  it("keeps the cached value and counts a failure when central fetch throws", async () => {
    const vault = newVault();
    const src = new FakeSource();
    // first pass succeeds, second pass throws
    src.set("db/pw", cs("v1", "cv-1"), new Error("network down"));
    const e = await SyncEngine.start(vault, src, "", [["db/pw", undefined]], 0, true);
    expect(vault.get("db/pw")!.asString()).toBe("v1");
    const successAt = e.stats().lastSuccessMs;

    await e.syncNow(); // throws internally → cached value preserved
    expect(vault.get("db/pw")!.asString()).toBe("v1"); // unchanged
    expect(e.stats().failures).toBe(1);
    // an all-failure pass must not advance lastSuccessMs
    expect(e.stats().lastSuccessMs).toBe(successAt);
    e.close();
  });

  it("does not set lastSuccessMs when the very first pass fails for every secret", async () => {
    const vault = newVault();
    const src = new FakeSource();
    src.set("db/pw", new Error("offline"));
    const e = await SyncEngine.start(vault, src, "", [["db/pw", undefined]], 0, true);
    expect(e.stats().failures).toBe(1);
    expect(e.stats().rotations).toBe(0);
    expect(e.stats().lastSuccessMs).toBeUndefined();
    expect(vault.get("db/pw")).toBeUndefined();
    e.close();
  });

  it("continues past a failing secret to sync the remaining ones (partial success)", async () => {
    const vault = newVault();
    const src = new FakeSource();
    src.set("a", new Error("boom"));
    src.set("b", cs("vb", "cv-b"));
    const e = await SyncEngine.start(
      vault,
      src,
      "",
      [
        ["a", undefined],
        ["b", undefined],
      ],
      0,
      true,
    );
    expect(vault.get("a")).toBeUndefined();
    expect(vault.get("b")!.asString()).toBe("vb");
    expect(e.stats().failures).toBe(1);
    expect(e.stats().rotations).toBe(1);
    expect(e.stats().lastSuccessMs).toBeGreaterThan(0); // partial success
    e.close();
  });
});

describe("sync engine concurrency + scheduling", () => {
  it("skips a concurrent syncNow while one is already running", async () => {
    const vault = newVault();
    const src = new FakeSource();
    let resolveFetch!: (v: CentralSecret) => void;
    const gate = new Promise<CentralSecret>((res) => (resolveFetch = res));
    src.fetch = vi.fn(() => gate);

    const e = await SyncEngine.start(vault, src, "", [["db/pw", undefined]], 0, false);
    const first = e.syncNow(); // begins, blocks on the gate
    const second = e.syncNow(); // should early-return (running guard)
    await second;
    expect(src.fetch).toHaveBeenCalledTimes(1);

    resolveFetch(cs("v1", "cv-1"));
    await first;
    expect(vault.get("db/pw")!.asString()).toBe("v1");
    e.close();
  });

  it("schedules a periodic refresh on a positive interval and stops it on close", async () => {
    vi.useFakeTimers();
    try {
      const vault = newVault();
      const src = new FakeSource();
      src.set("db/pw", cs("v1", "cv-1"));
      // bootstrap=false so the only fetches come from the interval timer
      const e = await SyncEngine.start(vault, src, "", [["db/pw", undefined]], 1, false);
      expect(src.fetch).not.toHaveBeenCalled();

      await vi.advanceTimersByTimeAsync(1000);
      expect(src.fetch).toHaveBeenCalledTimes(1);
      await vi.advanceTimersByTimeAsync(1000);
      expect(src.fetch).toHaveBeenCalledTimes(2);

      e.close();
      await vi.advanceTimersByTimeAsync(5000);
      expect(src.fetch).toHaveBeenCalledTimes(2); // no more fires after close
    } finally {
      vi.useRealTimers();
    }
  });

  it("does not schedule a timer when the interval is zero (manual refresh only)", async () => {
    vi.useFakeTimers();
    try {
      const vault = newVault();
      const src = new FakeSource();
      src.set("db/pw", cs("v1", "cv-1"));
      const e = await SyncEngine.start(vault, src, "", [["db/pw", undefined]], 0, false);
      await vi.advanceTimersByTimeAsync(10_000);
      expect(src.fetch).not.toHaveBeenCalled();
      e.close(); // close with no timer is a no-op
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("sync engine stats", () => {
  let warn: ReturnType<typeof vi.spyOn>;
  beforeEach(() => {
    // silence the offline-warning log noise from the failure paths
    warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
  });
  afterEach(() => warn.mockRestore());

  it("reports a fresh-engine snapshot", async () => {
    const vault = newVault();
    const e = await SyncEngine.start(vault, new FakeSource(), "", [], 0, false);
    expect(e.stats()).toEqual({ lastSuccessMs: undefined, failures: 0, rotations: 0 });
    e.close();
  });
});
