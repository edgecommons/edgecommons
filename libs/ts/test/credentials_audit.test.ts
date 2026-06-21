/** Credential access-audit (Phase 4) tests — events are emitted on get/put/delete and never
 * carry the secret value; a service with no sink is a no-op. Mirrors the Rust audit behavior. */
import { mkdtempSync } from "fs";
import { tmpdir } from "os";
import { join } from "path";

import { describe, expect, it } from "vitest";

import { AuditEvent, AuditSink } from "../src/credentials/audit";
import { FileKeyProvider } from "../src/credentials/keyprovider";
import { DefaultCredentialService } from "../src/credentials/service";
import { LocalVault } from "../src/credentials/vault";

/** A collecting sink that records every event for assertions. */
class CollectingSink implements AuditSink {
  readonly events: AuditEvent[] = [];
  record(event: AuditEvent): void {
    this.events.push(event);
  }
}

function vault(): LocalVault {
  const dir = mkdtempSync(join(tmpdir(), "ggaudit-"));
  const provider = new FileKeyProvider(Buffer.alloc(32, 7));
  return LocalVault.open(join(dir, "vault"), provider, 2);
}

function svc(sink?: AuditSink): DefaultCredentialService {
  return new DefaultCredentialService(vault(), "", undefined, sink);
}

const SECRET = "hunter2-super-secret-value";

describe("credential access audit", () => {
  it("emits put/get(hit)/get(miss)/delete events with expected fields", () => {
    const sink = new CollectingSink();
    const c = svc(sink);

    const version = c.put("db/password", Buffer.from(SECRET));
    expect(c.getString("db/password")).toBe(SECRET); // get hit
    expect(c.get("missing")).toBeUndefined(); // get miss
    expect(c.delete("db/password")).toBe(true); // delete ok
    expect(c.delete("missing")).toBe(false); // delete miss

    const e = sink.events;
    expect(e.length).toBe(5);

    expect(e[0]).toEqual({ op: "put", name: "db/password", version, source: "local", outcome: "ok" });

    expect(e[1].op).toBe("get");
    expect(e[1].name).toBe("db/password");
    expect(e[1].version).toBe(version);
    expect(e[1].source).toBe("local");
    expect(e[1].outcome).toBe("hit");

    expect(e[2]).toEqual({ op: "get", name: "missing", version: "-", source: "-", outcome: "miss" });
    expect(e[3]).toEqual({ op: "delete", name: "db/password", version: "-", source: "-", outcome: "ok" });
    expect(e[4]).toEqual({ op: "delete", name: "missing", version: "-", source: "-", outcome: "miss" });
  });

  it("getVersion emits a get hit/miss", () => {
    const sink = new CollectingSink();
    const c = svc(sink);
    const version = c.put("k", Buffer.from(SECRET));
    sink.events.length = 0;

    expect(c.getVersion("k", version)!.asString()).toBe(SECRET);
    expect(c.getVersion("k", "doesnotexist")).toBeUndefined();

    expect(sink.events[0]).toEqual({ op: "get", name: "k", version, source: "local", outcome: "hit" });
    expect(sink.events[1]).toEqual({ op: "get", name: "k", version: "doesnotexist", source: "-", outcome: "miss" });
  });

  it("never records the secret value in any event", () => {
    const sink = new CollectingSink();
    const c = svc(sink);
    c.put("a", Buffer.from(SECRET));
    c.get("a");
    c.getVersion("a", c.versions("a")[0]);
    c.delete("a");

    const serialized = JSON.stringify(sink.events);
    expect(serialized).not.toContain(SECRET);
    for (const e of sink.events) {
      expect(Object.values(e).join("|")).not.toContain(SECRET);
    }
  });

  it("is a no-op when no sink is configured (auditing off by default)", () => {
    const c = svc(); // no sink
    // None of these should throw despite no sink being attached.
    const version = c.put("a", Buffer.from(SECRET));
    expect(c.getString("a")).toBe(SECRET);
    expect(c.get("missing")).toBeUndefined();
    expect(c.getVersion("a", version)!.asString()).toBe(SECRET);
    expect(c.delete("a")).toBe(true);
  });

  it("withAudit attaches a sink fluently after construction", () => {
    const sink = new CollectingSink();
    const c = svc().withAudit(sink);
    c.put("a", Buffer.from(SECRET));
    expect(sink.events.length).toBe(1);
    expect(sink.events[0].op).toBe("put");

    c.withAudit(undefined);
    c.put("b", Buffer.from(SECRET));
    expect(sink.events.length).toBe(1); // unchanged: sink cleared
  });
});
