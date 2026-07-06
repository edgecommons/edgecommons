/**
 * Unit + loopback-integration tests for the Phase 1c health slice (`src/health.ts`):
 *
 *   - {@link ReadinessState}: the readyz_ok = connected && readyFlag && !shuttingDown model,
 *     `setReady`, and `beginShutdown`.
 *   - {@link evaluateHealth}: the pure routing/branch logic (livez always-200, readyz/startupz
 *     ready-vs-not, 404, 405).
 *   - {@link HealthServer}: real loopback GETs over an ephemeral port — `/livez` is 200 even when
 *     messaging is disconnected (a broker outage must NOT fail liveness), `/readyz` + `/startupz`
 *     flip 503→200 with readiness, unknown paths 404, non-GET 405.
 */
import * as http from "http";

import { describe, it, expect, afterEach } from "vitest";

import { HealthServer, ReadinessState, evaluateHealth, HealthPaths } from "../src/health";
import { DefaultMessagingService } from "../src/messaging/service";
import { FakeMessagingProvider } from "./_fakes";

const PATHS: HealthPaths = { liveness: "/livez", readiness: "/readyz", startup: "/startupz" };

/** Issue a loopback GET (or other method) and resolve {status, body}. */
function httpGet(port: number, path: string, method = "GET"): Promise<{ status: number; body: string }> {
  return new Promise((resolve, reject) => {
    const req = http.request({ host: "127.0.0.1", port, path, method }, (res) => {
      let body = "";
      res.on("data", (c) => (body += c));
      res.on("end", () => resolve({ status: res.statusCode ?? 0, body }));
    });
    req.on("error", reject);
    req.end();
  });
}

describe("ReadinessState", () => {
  it("is ready only when connected && readyFlag(default true) && !shuttingDown", () => {
    let connected = true;
    const r = new ReadinessState(() => connected);
    expect(r.isReady()).toBe(true); // readyFlag defaults true

    connected = false;
    expect(r.isReady()).toBe(false); // disconnected -> not ready
    connected = true;
    expect(r.isReady()).toBe(true);
  });

  it("setReady(false) gates readiness even while connected; setReady(true) restores it", () => {
    const r = new ReadinessState(() => true);
    r.setReady(false);
    expect(r.isReady()).toBe(false);
    r.setReady(true);
    expect(r.isReady()).toBe(true);
  });

  it("beginShutdown() flips readiness to not-ready (and is observable)", () => {
    const r = new ReadinessState(() => true);
    expect(r.isShuttingDown()).toBe(false);
    r.beginShutdown();
    expect(r.isShuttingDown()).toBe(true);
    expect(r.isReady()).toBe(false);
  });
});

describe("evaluateHealth (pure routing)", () => {
  const ready = new ReadinessState(() => true);
  const notReady = new ReadinessState(() => false);

  it("/livez is always 200 (never consults the broker)", () => {
    // Even with a never-ready state, liveness is 200 — the handler running is the liveness proof.
    expect(evaluateHealth("GET", "/livez", PATHS, notReady)).toEqual({ status: 200, body: "ok" });
  });

  it("/readyz reflects readiness", () => {
    expect(evaluateHealth("GET", "/readyz", PATHS, ready)).toEqual({ status: 200, body: "ok" });
    expect(evaluateHealth("GET", "/readyz", PATHS, notReady)).toEqual({ status: 503, body: "not ready" });
  });

  it("/startupz mirrors readiness", () => {
    expect(evaluateHealth("GET", "/startupz", PATHS, ready).status).toBe(200);
    expect(evaluateHealth("GET", "/startupz", PATHS, notReady).status).toBe(503);
  });

  it("strips the query string before matching", () => {
    expect(evaluateHealth("GET", "/readyz?probe=1", PATHS, ready).status).toBe(200);
  });

  it("unknown path -> 404", () => {
    expect(evaluateHealth("GET", "/nope", PATHS, ready)).toEqual({ status: 404, body: "not found" });
    expect(evaluateHealth("GET", undefined, PATHS, ready).status).toBe(404);
  });

  it("non-GET method -> 405", () => {
    expect(evaluateHealth("POST", "/livez", PATHS, ready)).toEqual({ status: 405, body: "method not allowed" });
    expect(evaluateHealth(undefined, "/livez", PATHS, ready).status).toBe(405);
  });

  it("honors custom configured paths", () => {
    const custom: HealthPaths = { liveness: "/alive", readiness: "/ready", startup: "/start" };
    expect(evaluateHealth("GET", "/alive", custom, notReady).status).toBe(200);
    expect(evaluateHealth("GET", "/ready", custom, notReady).status).toBe(503);
    expect(evaluateHealth("GET", "/livez", custom, notReady).status).toBe(404); // default path no longer routed
  });
});

describe("HealthServer (loopback over an ephemeral port)", () => {
  let server: HealthServer | undefined;

  afterEach(async () => {
    if (server) await server.stop();
    server = undefined;
  });

  it("serves /livez=200, /readyz and /startupz=503 before ready, 404 for unknown, 405 for non-GET", async () => {
    let connected = false; // start disconnected: NOT ready, but the process IS alive
    const readiness = new ReadinessState(() => connected);
    server = await HealthServer.start({ port: 0, paths: PATHS, readiness });
    const port = server.port();
    expect(port).toBeGreaterThan(0);

    // /livez is 200 even though messaging is disconnected (a broker outage must not fail liveness).
    expect(await httpGet(port, "/livez")).toEqual({ status: 200, body: "ok" });
    // /readyz + /startupz are 503 while disconnected.
    expect((await httpGet(port, "/readyz")).status).toBe(503);
    expect((await httpGet(port, "/startupz")).status).toBe(503);

    // Connect -> readiness flips to 200.
    connected = true;
    expect(await httpGet(port, "/readyz")).toEqual({ status: 200, body: "ok" });
    expect((await httpGet(port, "/startupz")).status).toBe(200);

    // beginShutdown -> 503 again even while connected.
    readiness.beginShutdown();
    expect((await httpGet(port, "/readyz")).status).toBe(503);

    // Unknown path -> 404; non-GET -> 405.
    expect((await httpGet(port, "/whatever")).status).toBe(404);
    expect((await httpGet(port, "/readyz", "POST")).status).toBe(405);
  });

  it("rejects when the port is already in use", async () => {
    const readiness = new ReadinessState(() => true);
    server = await HealthServer.start({ port: 0, paths: PATHS, readiness });
    const port = server.port();
    // A second bind on the same concrete port must fail.
    await expect(HealthServer.start({ port, paths: PATHS, readiness })).rejects.toBeTruthy();
  });
});

describe("graceful shutdown semantics (readiness wired to the real messaging service)", () => {
  it("connected() drives /readyz; beginShutdown flips to 503; disconnect unsubscribes all (idempotent)", async () => {
    const provider = new FakeMessagingProvider();
    const svc = new DefaultMessagingService(provider);
    const readiness = new ReadinessState(() => svc.connected());

    // Connected + readyFlag default true + not shutting down -> ready.
    expect(svc.connected()).toBe(true);
    await svc.subscribe("a/+", () => {});
    await svc.subscribeNorthbound("b/#", () => {});
    expect(provider.subs.length).toBe(2);
    expect(readiness.isReady()).toBe(true);

    // SIGTERM step 1: flip readiness to 503 BEFORE the drain.
    readiness.beginShutdown();
    expect(readiness.isReady()).toBe(false);

    // SIGTERM step 2: bounded close unsubscribes EVERY tracked subscription + drops the transport.
    await svc.disconnect();
    expect(provider.subs.length).toBe(0);
    expect(provider.disconnected).toBe(true);
    expect(svc.connected()).toBe(false); // disconnected transport -> still not ready

    // Idempotent: a second disconnect is a no-op and never throws.
    await expect(svc.disconnect()).resolves.toBeUndefined();
  });

  it("a disconnected transport alone (no shutdown) is enough to fail /readyz", () => {
    const provider = new FakeMessagingProvider();
    provider.connectedState = false; // simulate broker outage
    const svc = new DefaultMessagingService(provider);
    const readiness = new ReadinessState(() => svc.connected());
    // Not shutting down, readyFlag default true, but the broker is down -> NOT ready...
    expect(readiness.isReady()).toBe(false);
    // ...yet /livez is unaffected (the broker is never consulted for liveness).
    expect(evaluateHealth("GET", "/livez", PATHS, readiness).status).toBe(200);
  });
});
