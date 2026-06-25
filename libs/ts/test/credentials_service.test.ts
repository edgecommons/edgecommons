/** DefaultCredentialService unit tests: get/put/delete/list/versions, typed views and their error
 * branches, transparent namespacing, audit emission, and refresh/stats delegation to a SyncEngine.
 * Uses a real on-disk {@link LocalVault} and stub audit/sync collaborators (no AWS). */
import { mkdtempSync } from "fs";
import { tmpdir } from "os";
import { join } from "path";

import { describe, expect, it, vi } from "vitest";

import { AuditEvent, AuditSink } from "../src/credentials/audit";
import { CredentialError } from "../src/credentials/errors";
import { FileKeyProvider } from "../src/credentials/keyprovider";
import { DefaultCredentialService } from "../src/credentials/service";
import type { SyncEngine } from "../src/credentials/sync";
import { LocalVault } from "../src/credentials/vault";

function vault(): LocalVault {
  const dir = mkdtempSync(join(tmpdir(), "ggvault-svc-"));
  return LocalVault.open(join(dir, "vault"), new FileKeyProvider(Buffer.alloc(32, 3)), 2);
}

class RecordingSink implements AuditSink {
  readonly events: AuditEvent[] = [];
  record(e: AuditEvent): void {
    this.events.push(e);
  }
}

describe("DefaultCredentialService core ops", () => {
  it("put/get/exists/delete and getBytes/getString/getJson", () => {
    const c = new DefaultCredentialService(vault());
    const v = c.put("k", Buffer.from("val"));
    expect(v).toBe("00000001");
    expect(c.exists("k")).toBe(true);
    expect(c.getBytes("k")!.toString()).toBe("val");
    expect(c.getString("k")).toBe("val");

    c.put("j", Buffer.from('{"a":1}'));
    expect(c.getJson("j")).toEqual({ a: 1 });

    expect(c.delete("k")).toBe(true);
    expect(c.exists("k")).toBe(false);
    expect(c.delete("k")).toBe(false); // already gone
    expect(c.get("k")).toBeUndefined();
    expect(c.getBytes("k")).toBeUndefined();
    expect(c.getString("k")).toBeUndefined();
    expect(c.getJson("k")).toBeUndefined();
  });

  it("getVersion returns a specific version or undefined", () => {
    const c = new DefaultCredentialService(vault());
    c.put("k", Buffer.from("v1"));
    c.put("k", Buffer.from("v2"));
    expect(c.getVersion("k", "00000001")!.asString()).toBe("v1");
    expect(c.getVersion("k", "00000002")!.asString()).toBe("v2");
    expect(c.getVersion("k", "99999999")).toBeUndefined();
    expect(c.getVersion("missing", "00000001")).toBeUndefined();
    expect(c.versions("k")).toEqual(["00000001", "00000002"]);
    expect(c.versions("missing")).toEqual([]);
  });

  it("list returns metadata with caller-facing names", () => {
    const c = new DefaultCredentialService(vault());
    c.put("a", Buffer.from("1"));
    c.put("b", Buffer.from("2"));
    expect(c.list("").map((m) => m.name)).toEqual(["a", "b"]);
    expect(c.list("a").map((m) => m.name)).toEqual(["a"]);
  });
});

describe("DefaultCredentialService namespacing", () => {
  it("prepends the namespace on write and strips it from returned names", () => {
    const v = vault();
    const c = new DefaultCredentialService(v, "thing/Comp");
    c.put("db/pw", Buffer.from("s"));

    // caller-facing surface is un-namespaced
    expect(c.getString("db/pw")).toBe("s");
    expect(c.list("").map((m) => m.name)).toEqual(["db/pw"]);
    expect(c.versions("db/pw")).toEqual(["00000001"]);
    expect(c.getVersion("db/pw", "00000001")!.asString()).toBe("s");

    // but stored under the full namespaced key in the underlying vault
    expect(v.get("thing/Comp/db/pw")!.asString()).toBe("s");
    expect(v.get("db/pw")).toBeUndefined();
  });

  it("with an empty namespace stores keys verbatim", () => {
    const v = vault();
    const c = new DefaultCredentialService(v, "");
    c.put("plain", Buffer.from("x"));
    expect(v.get("plain")!.asString()).toBe("x");
  });
});

describe("DefaultCredentialService typed views", () => {
  function svc(): DefaultCredentialService {
    return new DefaultCredentialService(vault());
  }

  it("parses each typed view", () => {
    const c = svc();
    c.put("aws", Buffer.from('{"accessKeyId":"AK","secretAccessKey":"SK","sessionToken":"T","expiry":"2030"}'));
    c.put("basic", Buffer.from('{"username":"u","password":"p"}'));
    c.put("tls", Buffer.from('{"certPem":"C","keyPem":"K","caPem":"CA"}'));
    c.put("kafka", Buffer.from('{"mechanism":"SCRAM-SHA-512","username":"ku","password":"kp"}'));

    expect(c.getAwsCredentials("aws")).toEqual({
      accessKeyId: "AK",
      secretAccessKey: "SK",
      sessionToken: "T",
      expiry: "2030",
    });
    expect(c.getBasicAuth("basic")).toEqual({ username: "u", password: "p" });
    expect(c.getTlsBundle("tls")).toEqual({ certPem: "C", keyPem: "K", caPem: "CA" });
    expect(c.getKafkaSasl("kafka")).toEqual({ mechanism: "SCRAM-SHA-512", username: "ku", password: "kp" });
  });

  it("kafka mechanism defaults to PLAIN when absent or non-string", () => {
    const c = svc();
    c.put("k1", Buffer.from('{"username":"u","password":"p"}'));
    c.put("k2", Buffer.from('{"mechanism":7,"username":"u","password":"p"}'));
    expect(c.getKafkaSasl("k1")!.mechanism).toBe("PLAIN");
    expect(c.getKafkaSasl("k2")!.mechanism).toBe("PLAIN");
  });

  it("returns undefined for a missing secret on every typed view", () => {
    const c = svc();
    expect(c.getAwsCredentials("nope")).toBeUndefined();
    expect(c.getBasicAuth("nope")).toBeUndefined();
    expect(c.getTlsBundle("nope")).toBeUndefined();
    expect(c.getKafkaSasl("nope")).toBeUndefined();
  });

  it("throws CredentialError when a secret lacks the required fields for the view", () => {
    const c = svc();
    c.put("wrong", Buffer.from('{"unrelated":true}'));
    expect(() => c.getAwsCredentials("wrong")).toThrow(CredentialError);
    expect(() => c.getBasicAuth("wrong")).toThrow(CredentialError);
    expect(() => c.getTlsBundle("wrong")).toThrow(CredentialError);
    expect(() => c.getKafkaSasl("wrong")).toThrow(CredentialError);
  });

  it("propagates a non-JSON parse error from the underlying secret", () => {
    const c = svc();
    c.put("notjson", Buffer.from("not json at all"));
    expect(() => c.getAwsCredentials("notjson")).toThrow(CredentialError);
  });
});

describe("DefaultCredentialService auditing", () => {
  it("emits hit/miss/ok events and respects withAudit toggling", () => {
    const sink = new RecordingSink();
    const c = new DefaultCredentialService(vault()).withAudit(sink);

    c.put("k", Buffer.from("v")); // put → ok
    c.get("k"); // get → hit
    c.get("absent"); // get → miss
    c.getVersion("k", "00000001"); // getVersion hit
    c.getVersion("k", "00000009"); // getVersion miss
    c.delete("k"); // delete ok
    c.delete("k"); // delete miss

    const ops = sink.events.map((e) => `${e.op}:${e.outcome}`);
    expect(ops).toEqual([
      "put:ok",
      "get:hit",
      "get:miss",
      "get:hit",
      "get:miss",
      "delete:ok",
      "delete:miss",
    ]);
    expect(sink.events[1]).toMatchObject({ name: "k", source: "local" });

    // detach the sink — no further events
    const n = sink.events.length;
    c.withAudit(undefined).get("absent");
    expect(sink.events.length).toBe(n);
  });

  it("never records the secret value in audit events", () => {
    const sink = new RecordingSink();
    const c = new DefaultCredentialService(vault()).withAudit(sink);
    c.put("token", Buffer.from("super-secret-value"));
    c.get("token");
    expect(JSON.stringify(sink.events)).not.toContain("super-secret-value");
  });

  it("is a no-op when no audit sink is configured", () => {
    const c = new DefaultCredentialService(vault());
    expect(() => {
      c.put("k", Buffer.from("v"));
      c.get("k");
      c.delete("k");
    }).not.toThrow();
  });
});

describe("DefaultCredentialService sync delegation", () => {
  function fakeSync(over: Partial<SyncEngine> = {}): SyncEngine {
    return {
      syncNow: vi.fn(async () => undefined),
      stats: vi.fn(() => ({ lastSuccessMs: undefined, failures: 0, rotations: 0 })),
      close: vi.fn(),
      ...over,
    } as unknown as SyncEngine;
  }

  it("refresh() with no sync engine resolves without error", async () => {
    const c = new DefaultCredentialService(vault());
    await expect(c.refresh()).resolves.toBeUndefined();
  });

  it("refresh() delegates to the sync engine's syncNow", async () => {
    const sync = fakeSync();
    const c = new DefaultCredentialService(vault(), "", sync);
    await c.refresh();
    expect(sync.syncNow).toHaveBeenCalledOnce();
  });

  it("stats() with no sync engine reports zeroed sync counters", () => {
    const c = new DefaultCredentialService(vault());
    c.put("a", Buffer.from("1"));
    expect(c.stats()).toEqual({ secretCount: 1, syncFailures: 0, rotations: 0 });
  });

  it("stats() folds in sync counters and computes lastSyncAgeMs", () => {
    const now = Date.now();
    const sync = fakeSync({
      stats: vi.fn(() => ({ lastSuccessMs: now - 5000, failures: 2, rotations: 3 })),
    });
    const c = new DefaultCredentialService(vault(), "", sync);
    c.put("a", Buffer.from("1"));
    c.put("b", Buffer.from("2"));
    const s = c.stats();
    expect(s.secretCount).toBe(2);
    expect(s.syncFailures).toBe(2);
    expect(s.rotations).toBe(3);
    expect(s.lastSyncAgeMs).toBeGreaterThanOrEqual(5000);
  });

  it("stats() leaves lastSyncAgeMs undefined when sync never succeeded", () => {
    const sync = fakeSync({
      stats: vi.fn(() => ({ lastSuccessMs: undefined, failures: 1, rotations: 0 })),
    });
    const c = new DefaultCredentialService(vault(), "", sync);
    const s = c.stats();
    expect(s.lastSyncAgeMs).toBeUndefined();
    expect(s.syncFailures).toBe(1);
  });

  it("clamps a future lastSuccessMs to a non-negative age", () => {
    const sync = fakeSync({
      stats: vi.fn(() => ({ lastSuccessMs: Date.now() + 60_000, failures: 0, rotations: 0 })),
    });
    const c = new DefaultCredentialService(vault(), "", sync);
    expect(c.stats().lastSyncAgeMs).toBe(0);
  });
});
