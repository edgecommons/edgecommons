//! # Health — HTTP liveness/readiness endpoint + readiness state (FR-HB-1/2)
//!
//! **One-liner purpose**: A minimal, dependency-free HTTP/1.1 health server (`GET /livez`,
//! `/readyz`, `/startupz`) plus the thread-safe [`HealthState`] it serves, mirroring the
//! Kubernetes probe contract (DESIGN-subsystems §4).
//!
//! ## Overview
//! Two pieces:
//! 1. [`HealthState`] — the shared readiness state machine. `readyz_ok = messaging-connected &&
//!    ready && !shutting_down`. `ready` defaults to `true` (so a component is ready as soon as
//!    messaging connects); an app gates readiness on its own subscriptions by calling
//!    [`crate::EdgeCommons::set_ready`]. `shutting_down` is flipped by the library's SIGTERM watcher
//!    so `/readyz` returns 503 immediately on stop.
//! 2. [`HealthServer`] — a hand-rolled [`std::net::TcpListener`] responder running in a dedicated
//!    [`std::thread`] (no web framework, no extra crate, no new tokio feature). Dropping it stops
//!    the thread (RAII).
//!
//! ## Routes
//! - `GET <liveness_path>` (default `/livez`) → **200** while the process is alive. The handler
//!   running IS the liveness proof; it MUST NOT check the broker / any external dependency (a
//!   broker outage must never fail liveness, which would cause a restart storm).
//! - `GET <readiness_path>` (default `/readyz`) → **200** only when
//!   `connected && ready && !shutting_down`; otherwise **503**.
//! - `GET <startup_path>` (default `/startupz`) → reuses readiness semantics.
//! - any other path → **404**.
//!
//! The server binds `0.0.0.0:<port>` (default 8081). It is started only when health is enabled
//! (explicit `health.enabled` ▸ on by default on KUBERNETES ▸ off), wired in
//! [`crate::EdgeCommonsBuilder::build`].
//!
//! ## Safety & Panics
//! No `unsafe`. The accept loop never panics: malformed requests yield 400, I/O errors are logged
//! and retried. A bounded read timeout stops a slow client from wedging the single health thread.
//!
//! ## Related Modules
//! - [`crate::messaging`] (the [`crate::messaging::MessagingService::connected`] signal),
//!   [`crate::platform`] (the per-platform default), [`crate::config::model::HealthConfig`].

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::config::model::HealthConfig;
use crate::messaging::MessagingService;
use crate::platform::Platform;

/// How long the accept loop sleeps between non-blocking `accept()` polls. Bounds the shutdown
/// latency (the worst-case wait for the thread to notice the stop flag on drop).
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Read timeout for a single connection — a slow/garbage client cannot wedge the health thread.
const READ_TIMEOUT: Duration = Duration::from_secs(2);

/// The thread-safe readiness state shared by the health server, the SIGTERM watcher, and
/// [`crate::EdgeCommons::set_ready`].
///
/// Cloning shares the same underlying flags (`Arc<AtomicBool>`) and messaging handle, so a clone
/// handed to the server thread observes `set_ready` / shutdown changes made elsewhere.
#[derive(Clone)]
pub struct HealthState {
    /// The app-controllable readiness flag (defaults to `true`).
    ready: Arc<AtomicBool>,
    /// Required command-plane gate. Builder runtimes seed this false until acknowledged startup.
    command_plane_ready: Arc<AtomicBool>,
    /// Set `true` at the start of the shutdown/SIGTERM path so `/readyz` flips to 503 at once.
    shutting_down: Arc<AtomicBool>,
    /// Messaging handle for the connected() query; `None` when no messaging is wired (→ not ready).
    messaging: Option<Arc<dyn MessagingService>>,
}

impl HealthState {
    /// Build a fresh readiness state for the given (optional) messaging service.
    ///
    /// `ready` starts `true` and `shutting_down` starts `false`, so readiness is gated only by the
    /// messaging connection until the app calls [`Self::set_ready`] or shutdown begins.
    pub fn new(messaging: Option<Arc<dyn MessagingService>>) -> Self {
        Self::new_with_initial(messaging, true, true)
    }

    /// Build with explicit application and command-plane readiness seeds.
    ///
    /// Runtime construction uses `command_plane_ready = false` before the health endpoint starts,
    /// preventing a transient ready response before the command subscription is acknowledged.
    pub(crate) fn new_with_initial(
        messaging: Option<Arc<dyn MessagingService>>,
        initial_ready: bool,
        command_plane_ready: bool,
    ) -> Self {
        Self {
            ready: Arc::new(AtomicBool::new(initial_ready)),
            command_plane_ready: Arc::new(AtomicBool::new(command_plane_ready)),
            shutting_down: Arc::new(AtomicBool::new(false)),
            messaging,
        }
    }

    /// Set the app-controlled readiness flag (the `readyFlag` of the readiness model). Idempotent.
    pub fn set_ready(&self, ready: bool) {
        self.ready.store(ready, Ordering::SeqCst);
    }

    /// Update the required command-plane gate after STARTING/ACTIVE/FAILED/STOPPED transitions.
    pub(crate) fn set_command_plane_ready(&self, ready: bool) {
        self.command_plane_ready.store(ready, Ordering::SeqCst);
    }

    /// Mark the runtime as shutting down so `/readyz` returns 503 immediately. Idempotent.
    pub fn begin_shutdown(&self) {
        self.shutting_down.store(true, Ordering::SeqCst);
    }

    /// Whether shutdown has begun.
    pub fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::SeqCst)
    }

    /// Whether the messaging transport reports a live connection. `false` when no messaging is
    /// wired (an unwired runtime is not ready).
    pub fn messaging_connected(&self) -> bool {
        self.messaging
            .as_ref()
            .map(|m| m.connected())
            .unwrap_or(false)
    }

    /// `/livez`: always `true` here. Liveness is "the process/health-thread is running"; it
    /// deliberately does NOT consult the broker so an outage never triggers a restart storm.
    pub fn livez_ok(&self) -> bool {
        true
    }

    /// `/readyz` (and `/startupz`): `connected && ready && !shutting_down`.
    pub fn readyz_ok(&self) -> bool {
        self.messaging_connected()
            && self.ready.load(Ordering::SeqCst)
            && self.command_plane_ready.load(Ordering::SeqCst)
            && !self.shutting_down.load(Ordering::SeqCst)
    }
}

/// Resolve whether the health server should start (FR-HB-1, precedence FR-RT-3):
/// explicit `health.enabled` ▸ the platform-profile default (on for KUBERNETES, off elsewhere) ▸
/// `false`. The platform is known at build time, so no resolver→ConfigManager dependency is added.
pub fn resolve_enabled(config: &HealthConfig, platform: Platform) -> bool {
    config
        .enabled
        .unwrap_or_else(|| crate::platform::profile_health_enabled(platform))
}

/// The resolved HTTP health server settings (port + route paths). Built from
/// [`crate::config::model::HealthConfig`] in the runtime builder.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// TCP port to bind on `0.0.0.0` (default 8081). Use `0` in tests for an ephemeral port.
    pub port: u16,
    /// Liveness route (default `/livez`).
    pub liveness_path: String,
    /// Readiness route (default `/readyz`).
    pub readiness_path: String,
    /// Startup route (default `/startupz`; reuses readiness semantics).
    pub startup_path: String,
}

/// Owns the running HTTP health server thread. Dropping it stops the thread (RAII).
pub struct HealthServer {
    /// Set on drop to ask the accept loop to exit.
    stop: Arc<AtomicBool>,
    /// The actually-bound address (useful when binding port 0 in tests).
    addr: SocketAddr,
    /// The server thread handle, joined on drop.
    handle: Option<JoinHandle<()>>,
}

impl HealthServer {
    /// Bind `0.0.0.0:<port>` and spawn the responder thread.
    ///
    /// # Errors
    /// Returns the [`std::io::Error`] if the port cannot be bound (e.g. already in use) or the
    /// thread cannot be spawned. Binding happens synchronously here so [`Self::local_addr`] is
    /// valid as soon as this returns.
    pub fn start(config: ServerConfig, state: HealthState) -> std::io::Result<HealthServer> {
        let listener = TcpListener::bind(("0.0.0.0", config.port))?;
        let addr = listener.local_addr()?;
        // Non-blocking accept + a short poll lets the thread observe the stop flag without an extra
        // dependency or a self-connect trick.
        listener.set_nonblocking(true)?;

        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();
        let handle = std::thread::Builder::new()
            .name("edgecommons-health".to_string())
            .spawn(move || serve(listener, config, state, stop_thread))?;

        tracing::info!(addr = %addr, "health server listening");
        Ok(HealthServer {
            stop,
            addr,
            handle: Some(handle),
        })
    }

    /// The address the server is actually bound to (resolves the ephemeral port when bound to 0).
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }
}

impl Drop for HealthServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            // The accept loop notices the stop flag within POLL_INTERVAL, so this join is bounded.
            let _ = handle.join();
        }
    }
}

/// The accept loop: serve connections until the stop flag is set.
fn serve(listener: TcpListener, config: ServerConfig, state: HealthState, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _peer)) => handle_connection(stream, &config, &state),
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(POLL_INTERVAL);
            }
            Err(e) => {
                tracing::debug!(error = %e, "health server accept error");
                std::thread::sleep(POLL_INTERVAL);
            }
        }
    }
    tracing::debug!("health server stopped");
}

/// Read one request, route it, and write the response. Best-effort: any I/O error just drops the
/// connection (the kubelet retries on its probe interval).
fn handle_connection(mut stream: TcpStream, config: &ServerConfig, state: &HealthState) {
    // The listener is non-blocking; the accepted stream must be blocking for a timed read.
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(READ_TIMEOUT));

    let mut buf = [0u8; 1024];
    let n = match stream.read(&mut buf) {
        Ok(n) => n,
        Err(_) => return,
    };

    let (status, body) = match parse_request_target(&buf[..n]) {
        Some(path) => route(&path, config, state),
        None => (400, "bad request"),
    };
    let _ = write_response(&mut stream, status, body);
}

/// Extract the request-target path from the first request line (`GET /path?q=1 HTTP/1.1`),
/// stripping any query string. `None` if the bytes are not a parseable request line.
fn parse_request_target(raw: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(raw).ok()?;
    let first_line = text.lines().next()?;
    let mut parts = first_line.split_whitespace();
    let _method = parts.next()?; // Probes use GET; we route by path regardless of method.
    let target = parts.next()?;
    let path = target.split('?').next().unwrap_or(target);
    Some(path.to_string())
}

/// Route a request path to an `(http_status, body)`. Pure (no I/O) so the routing + readiness
/// decision is unit-testable in isolation.
fn route(path: &str, config: &ServerConfig, state: &HealthState) -> (u16, &'static str) {
    if path == config.liveness_path {
        // Liveness never checks the broker — the handler running is proof enough.
        (200, "ok")
    } else if path == config.readiness_path || path == config.startup_path {
        if state.readyz_ok() {
            (200, "ok")
        } else {
            (503, "not ready")
        }
    } else {
        (404, "not found")
    }
}

/// Write a minimal HTTP/1.1 response with a `Connection: close` so the client reads to EOF.
fn write_response(stream: &mut TcpStream, status: u16, body: &str) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        503 => "Service Unavailable",
        _ => "OK",
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/plain\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n{body}",
        len = body.len(),
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::RecordingMessaging;
    use std::time::Instant;

    /// A `ServerConfig` with the default routes, bound to an ephemeral port (0).
    fn test_config() -> ServerConfig {
        ServerConfig {
            port: 0,
            liveness_path: "/livez".to_string(),
            readiness_path: "/readyz".to_string(),
            startup_path: "/startupz".to_string(),
        }
    }

    /// Wrap a recorder in a `HealthState` (explicit `dyn` coercion through `Some`).
    fn state_with(messaging: Arc<RecordingMessaging>) -> HealthState {
        let svc: Arc<dyn MessagingService> = messaging;
        HealthState::new(Some(svc))
    }

    /// A state with a wired, connected messaging recorder (ready by default).
    fn connected_state() -> (HealthState, Arc<RecordingMessaging>) {
        let messaging = RecordingMessaging::new();
        messaging.set_connected(true);
        let state = state_with(messaging.clone());
        (state, messaging)
    }

    // ---------- enable precedence (FR-HB-1 / FR-RT-3) ----------

    #[test]
    fn health_off_by_default_on_host_and_greengrass() {
        let cfg = HealthConfig::default(); // enabled: None
        assert!(!resolve_enabled(&cfg, Platform::Host));
        assert!(!resolve_enabled(&cfg, Platform::Greengrass));
    }

    #[test]
    fn health_on_by_default_on_kubernetes() {
        let cfg = HealthConfig::default(); // enabled: None
        assert!(resolve_enabled(&cfg, Platform::Kubernetes));
    }

    #[test]
    fn explicit_health_enabled_overrides_platform_default() {
        // Explicit true turns it on even on HOST/GREENGRASS...
        let on = HealthConfig {
            enabled: Some(true),
            ..Default::default()
        };
        assert!(resolve_enabled(&on, Platform::Host));
        assert!(resolve_enabled(&on, Platform::Greengrass));
        // ...and explicit false turns it off even on KUBERNETES.
        let off = HealthConfig {
            enabled: Some(false),
            ..Default::default()
        };
        assert!(!resolve_enabled(&off, Platform::Kubernetes));
    }

    // ---------- readiness model (pure) ----------

    #[test]
    fn livez_is_always_ok_even_when_disconnected() {
        // Liveness must NOT depend on the broker: a disconnected runtime is still alive.
        let state = HealthState::new(None);
        assert!(state.livez_ok());
        let (_, msg) = connected_state();
        msg.set_connected(false);
        let state = state_with(msg);
        assert!(state.livez_ok(), "broker outage must not fail liveness");
    }

    #[test]
    fn readyz_is_503_before_connected() {
        let messaging = RecordingMessaging::new(); // not connected
        let state = state_with(messaging);
        assert!(!state.readyz_ok());
    }

    #[test]
    fn readyz_is_503_when_no_messaging_wired() {
        let state = HealthState::new(None);
        assert!(!state.readyz_ok(), "an unwired runtime is not ready");
    }

    #[test]
    fn explicit_initial_and_command_plane_gates_prevent_startup_ready_flicker() {
        let messaging = RecordingMessaging::new();
        messaging.set_connected(true);
        let svc: Arc<dyn MessagingService> = messaging;
        let state = HealthState::new_with_initial(Some(svc), false, false);

        assert!(!state.readyz_ok());
        state.set_command_plane_ready(true);
        assert!(!state.readyz_ok(), "application gate is still false");
        state.set_ready(true);
        assert!(state.readyz_ok());
        state.set_command_plane_ready(false);
        assert!(
            !state.readyz_ok(),
            "STOPPED/FAILED command plane must remove readiness"
        );
    }

    #[test]
    fn readyz_ok_when_connected_ready_and_not_shutting_down() {
        let (state, _msg) = connected_state();
        assert!(state.readyz_ok());
    }

    #[test]
    fn readyz_503_after_set_ready_false_then_200_again() {
        let (state, _msg) = connected_state();
        assert!(state.readyz_ok());
        state.set_ready(false);
        assert!(!state.readyz_ok(), "setReady(false) gates readiness");
        state.set_ready(true);
        assert!(state.readyz_ok(), "setReady(true) restores readiness");
    }

    #[test]
    fn readyz_503_when_shutting_down() {
        let (state, _msg) = connected_state();
        assert!(state.readyz_ok());
        state.begin_shutdown();
        assert!(state.is_shutting_down());
        assert!(!state.readyz_ok(), "shutdown flips readiness to 503");
        // Idempotent: a second begin_shutdown keeps it 503.
        state.begin_shutdown();
        assert!(!state.readyz_ok());
    }

    // ---------- routing (pure) ----------

    #[test]
    fn route_livez_is_200_regardless_of_readiness() {
        let cfg = test_config();
        let messaging = RecordingMessaging::new(); // disconnected → not ready
        let state = state_with(messaging);
        assert_eq!(route("/livez", &cfg, &state), (200, "ok"));
    }

    #[test]
    fn route_readyz_and_startupz_track_readiness() {
        let cfg = test_config();
        let (state, _msg) = connected_state();
        assert_eq!(route("/readyz", &cfg, &state), (200, "ok"));
        assert_eq!(route("/startupz", &cfg, &state), (200, "ok"));
        state.begin_shutdown();
        assert_eq!(route("/readyz", &cfg, &state), (503, "not ready"));
        assert_eq!(route("/startupz", &cfg, &state), (503, "not ready"));
    }

    #[test]
    fn route_unknown_path_is_404() {
        let cfg = test_config();
        let (state, _msg) = connected_state();
        assert_eq!(route("/nope", &cfg, &state), (404, "not found"));
        assert_eq!(route("/", &cfg, &state), (404, "not found"));
    }

    #[test]
    fn route_respects_custom_paths() {
        let cfg = ServerConfig {
            port: 0,
            liveness_path: "/alive".to_string(),
            readiness_path: "/ready".to_string(),
            startup_path: "/started".to_string(),
        };
        let (state, _msg) = connected_state();
        assert_eq!(route("/alive", &cfg, &state), (200, "ok"));
        assert_eq!(route("/ready", &cfg, &state), (200, "ok"));
        assert_eq!(route("/started", &cfg, &state), (200, "ok"));
        assert_eq!(
            route("/livez", &cfg, &state),
            (404, "not found"),
            "default paths off when remapped"
        );
    }

    // ---------- request parsing ----------

    #[test]
    fn parses_target_and_strips_query() {
        assert_eq!(
            parse_request_target(b"GET /livez HTTP/1.1\r\n\r\n").as_deref(),
            Some("/livez")
        );
        assert_eq!(
            parse_request_target(b"GET /readyz?probe=1 HTTP/1.1\r\n").as_deref(),
            Some("/readyz")
        );
        assert_eq!(parse_request_target(b"garbage"), None);
        assert_eq!(parse_request_target(b""), None);
    }

    // ---------- end-to-end over a real loopback socket ----------

    /// Issue a real `GET <path>` to the server and return `(status_code, body)`. The server binds
    /// the wildcard `0.0.0.0`, which clients cannot connect to (esp. on Windows: WSAEADDRNOTAVAIL),
    /// so dial the loopback interface on the server's bound port.
    fn http_get(addr: SocketAddr, path: &str) -> (u16, String) {
        let target = SocketAddr::from(([127, 0, 0, 1], addr.port()));
        let mut stream = TcpStream::connect(target).expect("connect to health server");
        stream
            .write_all(
                format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                    .as_bytes(),
            )
            .expect("send request");
        let mut response = String::new();
        stream.read_to_string(&mut response).expect("read response");
        let status: u16 = response
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|c| c.parse().ok())
            .expect("parse status code");
        let body = response.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
        (status, body)
    }

    #[test]
    fn server_serves_livez_readyz_startupz_and_404_over_loopback() {
        let (state, messaging) = connected_state();
        let server = HealthServer::start(test_config(), state.clone()).expect("start server");
        let addr = server.local_addr();

        // livez is 200 even when disconnected; flip connected off to prove liveness ignores it.
        messaging.set_connected(false);
        assert_eq!(http_get(addr, "/livez").0, 200, "livez ignores the broker");
        assert_eq!(
            http_get(addr, "/readyz").0,
            503,
            "readyz 503 while disconnected"
        );

        // Reconnect → ready.
        messaging.set_connected(true);
        assert_eq!(http_get(addr, "/readyz"), (200, "ok".to_string()));
        assert_eq!(http_get(addr, "/startupz").0, 200);

        // setReady(false) gates readiness; livez stays up.
        state.set_ready(false);
        assert_eq!(http_get(addr, "/readyz").0, 503);
        assert_eq!(http_get(addr, "/livez").0, 200);
        state.set_ready(true);
        assert_eq!(http_get(addr, "/readyz").0, 200);

        // shutdown flips readiness to 503 immediately.
        state.begin_shutdown();
        assert_eq!(http_get(addr, "/readyz").0, 503);
        assert_eq!(
            http_get(addr, "/livez").0,
            200,
            "livez stays 200 during shutdown"
        );

        // unknown path → 404.
        assert_eq!(http_get(addr, "/nope").0, 404);
    }

    #[test]
    fn server_thread_stops_on_drop() {
        let (state, _msg) = connected_state();
        let server = HealthServer::start(test_config(), state).expect("start server");
        let addr = server.local_addr();
        assert_eq!(http_get(addr, "/livez").0, 200);

        drop(server);

        // After drop the listener is closed; a fresh bind on the same port must eventually succeed,
        // proving the server thread released the socket (bounded by the accept poll interval).
        let mut bound = false;
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if TcpListener::bind(addr).is_ok() {
                bound = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(bound, "health server did not release its port after drop");
    }
}
