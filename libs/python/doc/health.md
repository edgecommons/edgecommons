# Health Server & Graceful Shutdown

The health subsystem (Phase 1c) adds a minimal, dependency-free HTTP health endpoint and wires
`SIGTERM` to the existing graceful-shutdown path. It is aimed at Kubernetes liveness/readiness/startup
probes but works on any platform.

## HTTP health endpoint (FR-HB-1)

A standard-library `http.server.ThreadingHTTPServer` runs on a daemon thread (no web framework). It
binds `0.0.0.0` on the configured port (default `8081`) and serves three GET routes:

| Route       | Default path | Behavior |
|-------------|--------------|----------|
| liveness    | `/livez`     | `200 ok` while the process is alive. **Never** checks the broker — a broker/cloud outage must not fail liveness (that would cause restart storms). |
| readiness   | `/readyz`    | `200 ok` only when `messagingConnected && readyFlag && commandInboxActive && !shuttingDown`; otherwise `503 not ready`. |
| startup     | `/startupz`  | Reuses the readiness semantics (for slow connects). |

Any other path returns `404 not found`.

## Readiness model

Readiness is tracked in a thread-safe `ReadinessState`:

- **messagingConnected** — queried live via `MessagingClient.connected()` (which delegates to the
  provider's `connected()`: paho `is_connected()` for MQTT, "IPC client built" for Greengrass). If no
  messaging is wired it reports `False` (not ready).
- **readyFlag** — defaults to `True`. Components with mandatory startup gates select
  `EdgeCommonsBuilder.initial_ready(False)`, which is applied before parsing, transport/config startup,
  or the health endpoint; they call `gg.set_ready(True)` only after their gates pass.
- **commandInboxActive** — the command plane must report `ACTIVE`, meaning every built-in/component
  handler is installed and MQTT SUBACK or the Greengrass subscription operation succeeded. A later
  `FAILED`/`STOPPED` transition immediately makes readiness false.
- **shuttingDown** — latched at the start of the shutdown/SIGTERM path so `/readyz` flips to `503`
  immediately.

## Enable / disable (FR-RT-3 precedence)

The server is enabled by: **explicit `health.enabled`** ▸ **on by default on KUBERNETES** ▸ **off**.
So on KUBERNETES it starts with no config; on GREENGRASS/HOST it is off unless `health.enabled: true`.

```jsonc
"health": {
  "enabled": true,            // optional; omit to use the platform default
  "port": 8081,
  "livenessPath": "/livez",
  "readinessPath": "/readyz",
  "startupPath": "/startupz"
}
```

## SIGTERM → graceful shutdown (FR-HB-2)

The library installs a `SIGTERM` handler on the main thread that:

1. flips `shuttingDown=true` (so `/readyz` → `503` immediately),
2. runs the idempotent `shutdown()` (unsubscribe every tracked subscription + bounded-close
   messaging/streams/heartbeat/vault and stop the health server),
3. exits `0`.

The handler is installed only when running on the main thread (`signal.signal` raises off the main
thread); otherwise the app keeps responsibility for calling `gg.shutdown()`. The previous `SIGTERM`
handler is restored on shutdown so the library does not permanently hijack signals. Because the
library now owns `SIGTERM`, components no longer need their own handler — `gg.shutdown()` is called
for them. `shutdown()` is idempotent, so a `try/finally: gg.shutdown()` in the app is still safe.
