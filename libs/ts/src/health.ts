/**
 * Health — a minimal, dependency-free HTTP health/readiness endpoint plus the
 * thread-safe readiness state behind it (Phase 1c / FR-HB-1).
 *
 * **One-liner purpose**: expose Kubernetes-style probes (`GET /livez`, `/readyz`,
 * `/startupz`) over node's built-in `http` module (no web framework), driven by a
 * small {@link ReadinessState} the lifecycle owns. On by default on the KUBERNETES
 * platform; opt-in elsewhere via the `health` config section. Mirrors the
 * Java/Python/Rust health slice.
 *
 * ## Probe semantics (locked design — all four langs must match)
 * - `GET /livez` → **always 200** while the process/event-loop is alive (the handler
 *   running IS the liveness proof). It MUST NOT consult the broker / any external
 *   dependency: a broker outage must never fail liveness, or the kubelet would restart
 *   the pod in a storm.
 * - `GET /readyz` → 200 **only** when `messagingConnected && readyFlag && !shuttingDown`
 *   (see {@link ReadinessState.isReady}); 503 otherwise (startup, disconnect, app gated
 *   not-ready, or shutting down).
 * - `GET /startupz` → reuses the readiness semantics (200 when ready, else 503), for
 *   slow connects.
 * - any other path → 404; any non-GET method → 405.
 *
 * The server binds `0.0.0.0` on the configured port (default 8081). Bodies are minimal
 * (`ok` / `ready` / `not ready`). The handler is split out as the pure
 * {@link evaluateHealth} so the routing/readiness logic is unit-testable without a socket.
 */
import * as http from "http";
import type { AddressInfo } from "net";

import { logger } from "./logging";

/**
 * The thread-safe (single-threaded-Node-trivial) readiness state the lifecycle owns and the `/readyz`
 * probe reads. Combines three inputs:
 *
 * - `messagingConnected` — a live getter onto the messaging transport's connection state (the runtime
 *   passes `() => messaging?.connected() ?? false`, so no wired messaging service ⇒ not ready);
 * - `readyFlag` — an app-settable boolean (defaults `true`) so a component is ready as soon as it is
 *   connected, but an app can gate readiness on its own required subscriptions by calling
 *   `setReady(false)` early and `setReady(true)` after subscribing;
 * - `shuttingDown` — flipped `true` at the start of the shutdown/SIGTERM path so `/readyz` flips to 503
 *   immediately (before the drain), even though the process is still alive.
 */
export class ReadinessState {
  private readyFlag = true;
  /** Library-owned readiness gates, kept separate from the application's ready flag. */
  private dependenciesReady = true;
  private shuttingDown = false;

  /**
   * @param messagingConnected a cheap, non-blocking getter for the messaging transport's connected
   *        state (the runtime supplies `() => messaging?.connected() ?? false`).
   */
  constructor(private readonly messagingConnected: () => boolean) {}

  /**
   * Set the app-controlled readiness flag (FR-HB-1). Defaults to `true`. Set `false` early (e.g. before
   * subscribing to required topics) to keep `/readyz` at 503, then `true` once the component can serve.
   */
  setReady(ready: boolean): void {
    this.readyFlag = ready;
  }

  /** @internal Set readiness of required library-owned infrastructure. */
  setDependenciesReady(ready: boolean): void {
    this.dependenciesReady = ready;
  }

  /** Begin shutdown: flip the shutting-down flag so `/readyz` returns 503 immediately (FR-HB-2). */
  beginShutdown(): void {
    this.shuttingDown = true;
  }

  /** Whether the runtime has entered the shutdown path. */
  isShuttingDown(): boolean {
    return this.shuttingDown;
  }

  /**
   * Whether the runtime is ready to serve traffic (the `/readyz` and `/startupz` signal):
   * `messagingConnected() && readyFlag && !shuttingDown`.
   */
  isReady(): boolean {
    return this.messagingConnected() && this.readyFlag && this.dependenciesReady && !this.shuttingDown;
  }
}

/** The resolved probe paths (defaults `/livez`, `/readyz`, `/startupz`; overridable via config). */
export interface HealthPaths {
  liveness: string;
  readiness: string;
  startup: string;
}

/** A computed HTTP response: status code + minimal plain-text body. */
export interface HealthResponse {
  status: number;
  body: string;
}

/**
 * Pure routing + readiness evaluation for one request (no I/O). Split out from {@link HealthServer} so
 * every branch (livez always-200, readyz/startupz ready vs not-ready, 404, 405) is unit-testable
 * without binding a socket.
 *
 * @param method the HTTP method (only `GET` is served; anything else → 405).
 * @param url the request target (the query string, if any, is stripped before matching).
 * @param paths the resolved probe paths.
 * @param readiness the live readiness state.
 */
export function evaluateHealth(
  method: string | undefined,
  url: string | undefined,
  paths: HealthPaths,
  readiness: ReadinessState,
): HealthResponse {
  if (method !== "GET") {
    return { status: 405, body: "method not allowed" };
  }
  const path = (url ?? "").split("?")[0];
  // /livez: the process is alive (this handler ran). NEVER consult the broker here.
  if (path === paths.liveness) {
    return { status: 200, body: "ok" };
  }
  // /readyz and /startupz share the readiness semantics.
  if (path === paths.readiness || path === paths.startup) {
    return readiness.isReady() ? { status: 200, body: "ok" } : { status: 503, body: "not ready" };
  }
  return { status: 404, body: "not found" };
}

/** Construction options for {@link HealthServer.start}. */
export interface HealthServerOptions {
  /** TCP port to bind on `0.0.0.0` (use `0` for an ephemeral port in tests). */
  port: number;
  /** The resolved probe paths. */
  paths: HealthPaths;
  /** The shared readiness state. */
  readiness: ReadinessState;
}

/**
 * The minimal HTTP/1.1 health server over node's built-in `http` module (no framework, no new
 * dependency). Start it with {@link start} (resolves once listening); stop it with {@link stop} during
 * shutdown. The listener is `unref`'d so it never keeps the process alive on its own — the app's run
 * loop owns process lifetime (mirrors the heartbeat timer).
 */
export class HealthServer {
  private constructor(private readonly server: http.Server) {}

  /**
   * Bind the health server on `0.0.0.0:<port>` and resolve once it is listening (or reject if the bind
   * fails, e.g. the port is in use). Each request is routed by {@link evaluateHealth}.
   */
  static start(opts: HealthServerOptions): Promise<HealthServer> {
    const { paths, readiness } = opts;
    const server = http.createServer((req, res) => {
      const { status, body } = evaluateHealth(req.method, req.url, paths, readiness);
      res.writeHead(status, { "Content-Type": "text/plain; charset=utf-8" });
      res.end(body);
    });
    // Never keep the event loop alive solely for the health server (parity with the heartbeat timer).
    server.unref();
    return new Promise<HealthServer>((resolve, reject) => {
      const onError = (err: Error): void => reject(err);
      server.once("error", onError);
      server.listen(opts.port, "0.0.0.0", () => {
        server.removeListener("error", onError);
        const addr = server.address() as AddressInfo | null;
        const boundPort = addr ? addr.port : opts.port;
        logger.info(
          `health server listening on 0.0.0.0:${boundPort} ` +
            `(livez=${paths.liveness} readyz=${paths.readiness} startupz=${paths.startup})`,
        );
        resolve(new HealthServer(server));
      });
    });
  }

  /** The actually-bound port (useful when started on the ephemeral port `0` in tests). */
  port(): number {
    const addr = this.server.address();
    return addr && typeof addr === "object" ? addr.port : 0;
  }

  /** Stop accepting connections and close the server. Idempotent-safe (close after close is a no-op). */
  stop(): Promise<void> {
    return new Promise<void>((resolve) => {
      this.server.close(() => resolve());
    });
  }
}
