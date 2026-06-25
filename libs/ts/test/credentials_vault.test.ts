/**
 * Unit tests for {@link LocalVault} internals — the on-disk store behind the credential service.
 *
 * Drives the vault directly (not through DefaultCredentialService) to exercise version pruning,
 * prefix listing, cross-process reload-on-change, delete semantics, PutOptions plumbing,
 * latestCentralVersionId, and the fail-closed I/O / format / lock error paths.
 */
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmdirSync, writeFileSync } from "fs";
import { tmpdir } from "os";
import { join } from "path";

import { afterEach, describe, expect, it } from "vitest";

import { CredentialError } from "../src/credentials/errors";
import { FileKeyProvider } from "../src/credentials/keyprovider";
import { LocalVault } from "../src/credentials/vault";

function tmpVaultPath(): string {
  return join(mkdtempSync(join(tmpdir(), "ggvault-")), "vault");
}

const KEY = new FileKeyProvider(Buffer.alloc(32, 7));
function provider(): FileKeyProvider {
  return new FileKeyProvider(Buffer.alloc(32, 7));
}

const leftoverLocks: string[] = [];
afterEach(() => {
  for (const d of leftoverLocks.splice(0)) {
    try {
      rmdirSync(d);
    } catch {
      /* best effort */
    }
  }
});

describe("LocalVault basics", () => {
  it("put/get roundtrip; get/exists report absence", () => {
    const v = LocalVault.open(tmpVaultPath(), provider(), 3);
    v.put("a", Buffer.from("alpha"));
    expect(v.get("a")!.bytes().toString()).toBe("alpha");
    expect(v.exists("a")).toBe(true);
    expect(v.get("missing")).toBeUndefined();
    expect(v.exists("missing")).toBe(false);
  });

  it("getVersion returns the right version, undefined for unknown version, undefined for unknown name", () => {
    const v = LocalVault.open(tmpVaultPath(), provider(), 5);
    v.put("k", Buffer.from("v1"));
    v.put("k", Buffer.from("v2"));
    expect(v.getVersion("k", "00000001")!.bytes().toString()).toBe("v1");
    expect(v.getVersion("k", "00000002")!.bytes().toString()).toBe("v2");
    expect(v.getVersion("k", "99999999")).toBeUndefined(); // version not present
    expect(v.getVersion("absent", "00000001")).toBeUndefined(); // name not present
  });

  it("versions() lists present versions and is empty for an unknown name", () => {
    const v = LocalVault.open(tmpVaultPath(), provider(), 5);
    v.put("k", Buffer.from("v1"));
    v.put("k", Buffer.from("v2"));
    expect(v.versions("k")).toEqual(["00000001", "00000002"]);
    expect(v.versions("absent")).toEqual([]);
  });

  it("prunes to keepVersions (min 1) keeping the newest", () => {
    const v = LocalVault.open(tmpVaultPath(), provider(), 1); // keep newest only
    v.put("k", Buffer.from("v1"));
    v.put("k", Buffer.from("v2"));
    v.put("k", Buffer.from("v3"));
    expect(v.versions("k")).toEqual(["00000003"]);
    expect(v.get("k")!.bytes().toString()).toBe("v3");
  });

  it("treats keepVersions <= 0 as 1", () => {
    const v = LocalVault.open(tmpVaultPath(), provider(), 0);
    v.put("k", Buffer.from("v1"));
    v.put("k", Buffer.from("v2"));
    expect(v.versions("k")).toEqual(["00000002"]);
  });

  it("list() sorts by name bytes and filters by prefix", () => {
    const v = LocalVault.open(tmpVaultPath(), provider(), 2);
    v.put("svc/b", Buffer.from("1"));
    v.put("svc/a", Buffer.from("2"));
    v.put("other", Buffer.from("3"));
    expect(v.list("").map((m) => m.name)).toEqual(["other", "svc/a", "svc/b"]);
    expect(v.list("svc/").map((m) => m.name)).toEqual(["svc/a", "svc/b"]);
    expect(v.list("zzz")).toEqual([]); // prefix matches nothing
    const meta = v.list("svc/a")[0];
    expect(meta).toMatchObject({ name: "svc/a", version: "00000001", source: "local" });
  });

  it("delete removes an existing secret and reports false for a missing one", () => {
    const v = LocalVault.open(tmpVaultPath(), provider(), 2);
    v.put("k", Buffer.from("v"));
    expect(v.delete("k")).toBe(true);
    expect(v.exists("k")).toBe(false);
    expect(v.delete("k")).toBe(false); // already gone
    expect(v.delete("never")).toBe(false);
  });
});

describe("LocalVault PutOptions", () => {
  it("records ttl, labels, contentType, source, and centralVersionId", () => {
    const v = LocalVault.open(tmpVaultPath(), provider(), 2);
    v.put("k", Buffer.from("v"), {
      ttlSecs: 60,
      labels: { env: "prod" },
      contentType: "application/json",
      source: "central",
      centralVersionId: "cv-1",
    });
    const meta = v.list("")[0];
    expect(meta.ttlSecs).toBe(60);
    expect(meta.labels).toEqual({ env: "prod" });
    expect(meta.source).toBe("central");
    const s = v.get("k")!;
    expect(s.contentType).toBe("application/json");
    expect(s.source).toBe("central");
    expect(v.latestCentralVersionId("k")).toBe("cv-1");
  });

  it("uses defaults when options are omitted and empty labels are dropped", () => {
    const v = LocalVault.open(tmpVaultPath(), provider(), 2);
    v.put("k", Buffer.from("v"), { labels: {} }); // empty labels -> not stored
    const s = v.get("k")!;
    expect(s.source).toBe("local");
    expect(s.contentType).toBe("application/octet-stream");
    expect(s.labels).toEqual({});
    expect(v.latestCentralVersionId("k")).toBeUndefined();
    expect(v.latestCentralVersionId("absent")).toBeUndefined();
  });
});

describe("LocalVault persistence and reload", () => {
  it("persists and reopens with the same key", () => {
    const path = tmpVaultPath();
    LocalVault.open(path, provider(), 2).put("tok", Buffer.from("abc"));
    expect(LocalVault.open(path, provider(), 2).get("tok")!.bytes().toString()).toBe("abc");
  });

  it("reloadIfChanged picks up another writer and is a no-op when unchanged", () => {
    const path = tmpVaultPath();
    const a = LocalVault.open(path, provider(), 5);
    a.put("k", Buffer.from("v1"));

    // Second handle on the same file (separate writer / different process simulation).
    const b = LocalVault.open(path, provider(), 5);
    expect(b.reloadIfChanged()).toBe(false); // nothing changed since open

    a.put("k", Buffer.from("v2"));
    expect(b.get("k")!.bytes().toString()).toBe("v1"); // stale until reload
    expect(b.reloadIfChanged()).toBe(true);
    expect(b.get("k")!.bytes().toString()).toBe("v2");
    expect(b.reloadIfChanged()).toBe(false); // up to date again
  });
});

describe("LocalVault fail-closed paths", () => {
  it("wrong KEK fails the MAC/AEAD check on open", () => {
    const path = tmpVaultPath();
    LocalVault.open(path, provider(), 2).put("k", Buffer.from("v"));
    expect(() => LocalVault.open(path, new FileKeyProvider(Buffer.alloc(32, 9)), 2)).toThrow(
      CredentialError,
    );
  });

  it("rejects an unsupported format version", () => {
    const path = tmpVaultPath();
    LocalVault.open(path, provider(), 2).put("k", Buffer.from("v"));
    const vf = JSON.parse(readFileSync(path, "utf8"));
    vf.format = 999;
    writeFileSync(path, JSON.stringify(vf));
    expect(() => LocalVault.open(path, provider(), 2)).toThrow(/unsupported vault format 999/);
  });

  it("wraps a read failure when the vault path is a directory", () => {
    // existsSync is true for a directory, but readFileSync(dir) throws EISDIR -> read vault error.
    const dir = mkdtempSync(join(tmpdir(), "ggvault-"));
    const path = join(dir, "asdir");
    mkdirSync(path);
    expect(() => LocalVault.open(path, provider(), 2)).toThrow(/read vault/);
  });

  it("rejects unparseable JSON on disk", () => {
    const path = tmpVaultPath();
    LocalVault.open(path, provider(), 2); // create a valid (empty) vault dir
    writeFileSync(path, "{ this is not json");
    expect(() => LocalVault.open(path, provider(), 2)).toThrow(/parse vault/);
  });

  it("detects a tampered ciphertext byte (MAC failure)", () => {
    const path = tmpVaultPath();
    LocalVault.open(path, provider(), 2).put("k", Buffer.from("v1"));
    const vf = JSON.parse(readFileSync(path, "utf8"));
    const ct = Buffer.from(vf.secrets.k.versions[0].ciphertext, "base64");
    ct[0] ^= 1;
    vf.secrets.k.versions[0].ciphertext = ct.toString("base64");
    writeFileSync(path, JSON.stringify(vf));
    expect(() => LocalVault.open(path, provider(), 2)).toThrow(/integrity check failed/);
  });
});

describe("LocalVault write lock", () => {
  // withLock retries for 5s before giving up, so allow more than vitest's default 5s timeout.
  it(
    "times out when the lock directory is held by another process",
    () => {
      const path = tmpVaultPath();
      const v = LocalVault.open(path, provider(), 2);
      // Pre-create the lock dir so withLock() can never mkdir it -> bounded retry then timeout.
      const lockDir = `${path}.lock`;
      mkdirSync(lockDir);
      leftoverLocks.push(lockDir);
      expect(() => v.put("k", Buffer.from("v"))).toThrow(/timed out acquiring vault lock/);
    },
    10_000,
  );
});

describe("LocalVault cross-language conformance", () => {
  const VECTORS = join(__dirname, "..", "..", "..", "vault-test-vectors");
  it.skipIf(!existsSync(join(VECTORS, "vault.json")))(
    "opens the Rust-generated canonical vault and recovers both secrets",
    () => {
      const p = FileKeyProvider.fromKeyFile(join(VECTORS, "vault.key"));
      const v = LocalVault.open(join(VECTORS, "vault.json"), p, 2);
      expect(v.get("alpha")!.bytes().toString("utf8")).toBe("hello");
      expect((v.get("beta")!.asJson() as { x: number }).x).toBe(1);
      expect(v.list("").map((m) => m.name)).toEqual(["alpha", "beta"]);
      void KEY;
    },
  );
});
