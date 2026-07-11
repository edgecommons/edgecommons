"""
Minimal, dependency-free HTTP health server for the Kubernetes probes (Phase 1c, FR-HB-1).

Implemented on the standard-library :class:`http.server.ThreadingHTTPServer` /
:class:`http.server.BaseHTTPRequestHandler` running on a daemon thread — no web framework, no extra
dependency. It serves three GET routes (paths configurable):

* ``GET /livez``   -> ``200 ok`` **while the process is alive**. The handler running *is* the liveness
  proof; it deliberately **never** queries the broker or any external dependency, so a broker/cloud
  outage cannot fail liveness and trigger a restart storm (FR-HB-1, DESIGN-subsystems §4).
* ``GET /readyz``  -> ``200 ok`` only when ``messagingConnected && readyFlag && !shuttingDown``; else
  ``503 not ready``. See :class:`ReadinessState`.
* ``GET /startupz`` -> reuses the readiness semantics (200 when ready, else 503).

Any other path -> ``404 not found``. The server binds ``0.0.0.0`` on the configured port (default
8081). Mirrors the canonical Java ``com.sun.net.httpserver.HttpServer`` health server and the Rust/TS
equivalents for four-way parity.
"""

import logging
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Callable, Optional

from edgecommons.config.health_config import HealthConfiguration

logger = logging.getLogger("HealthServer")


class ReadinessState:
    """Thread-safe readiness state backing ``/readyz`` and ``/startupz`` (DESIGN-subsystems §4).

    Readiness is ``messagingConnected && readyFlag && !shuttingDown``:

    * **messagingConnected** is queried lazily through ``connected_fn`` (the messaging layer's
      ``connected()`` accessor) so the probe always reflects the *current* link state; if no messaging
      is wired the callable reports ``False`` (not ready).
    * **readyFlag** defaults to ``True`` and is flipped by the app via :meth:`set_ready` — a component
      is ready as soon as messaging connects, but an app may gate readiness on its own required
      subscriptions by calling ``set_ready(False)`` early and ``set_ready(True)`` once subscribed.
    * **shuttingDown** is latched by :meth:`set_shutting_down` at the start of the shutdown/SIGTERM
      path so ``/readyz`` flips to 503 immediately (FR-HB-2).

    Liveness does **not** consult this object — it is unconditionally alive while the handler runs.
    """

    def __init__(
        self,
        connected_fn: Callable[[], bool],
        initial_ready: bool = True,
        required_ready_fn: Optional[Callable[[], bool]] = None,
    ):
        """
        Args:
            connected_fn: a zero-arg callable returning the messaging layer's connected state. It is
                invoked on every readiness check (outside the lock) and must be cheap and non-blocking.
        """
        self._connected_fn = connected_fn
        self._required_ready_fn = required_ready_fn
        self._ready_flag = bool(initial_ready)
        self._shutting_down = False
        self._lock = threading.Lock()

    def set_ready(self, ready: bool) -> None:
        """Set the app-controlled readiness flag (``gg.set_ready(...)``)."""
        with self._lock:
            self._ready_flag = bool(ready)

    def set_shutting_down(self) -> None:
        """Latch the shutting-down flag so ``/readyz`` reports 503 from now on (idempotent)."""
        with self._lock:
            self._shutting_down = True

    def is_shutting_down(self) -> bool:
        """Whether the shutdown path has begun."""
        with self._lock:
            return self._shutting_down

    def is_ready(self) -> bool:
        """``True`` only when connected AND the ready flag is set AND not shutting down."""
        with self._lock:
            if self._shutting_down or not self._ready_flag:
                return False
        # Query the messaging connection outside the lock so a slow/odd accessor can never deadlock
        # readiness checks; treat any error as "not connected" (fail closed -> 503).
        try:
            connected = bool(self._connected_fn())
            required = (
                True
                if self._required_ready_fn is None
                else bool(self._required_ready_fn())
            )
            return connected and required
        except Exception as e:  # noqa: BLE001 - readiness must never raise out of the handler
            logger.debug("readiness connected() check failed; treating as not connected: %s", e)
            return False


class _HealthRequestHandler(BaseHTTPRequestHandler):
    """Routes the three probe paths; everything else is 404. Bodies are tiny and allocation-light."""

    # Keep the Server: header generic and avoid leaking the Python version.
    server_version = "edgecommons-health"
    sys_version = ""
    # HTTP/1.1 but we always send Content-Length and do not keep connections open implicitly.
    protocol_version = "HTTP/1.1"

    def do_GET(self):  # noqa: N802 - name mandated by BaseHTTPRequestHandler
        srv = self.server
        path = self.path.split("?", 1)[0]  # ignore any query string
        if path == srv.liveness_path:
            # Liveness: alive iff this handler is running. MUST NOT check the broker.
            self._respond(200, b"ok")
        elif path == srv.readiness_path or path == srv.startup_path:
            if srv.readiness.is_ready():
                self._respond(200, b"ok")
            else:
                self._respond(503, b"not ready")
        else:
            self._respond(404, b"not found")

    def _respond(self, status: int, body: bytes) -> None:
        self.send_response(status)
        self.send_header("Content-Type", "text/plain; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        try:
            self.wfile.write(body)
        except (BrokenPipeError, ConnectionResetError):
            # The prober closed the socket early (kubelet does this); not worth logging at WARNING.
            pass

    def log_message(self, fmt, *args):  # noqa: A003 - override of BaseHTTPRequestHandler
        # Route the default stderr access log to DEBUG so probes don't spam the component log.
        logger.debug("health %s %s", self.address_string(), fmt % args)


class _HealthHTTPServer(ThreadingHTTPServer):
    """A threaded HTTP server carrying the readiness state and resolved probe paths."""

    daemon_threads = True
    allow_reuse_address = True

    def __init__(
        self,
        server_address,
        readiness: ReadinessState,
        liveness_path: str,
        readiness_path: str,
        startup_path: str,
    ):
        super().__init__(server_address, _HealthRequestHandler)
        self.readiness = readiness
        self.liveness_path = liveness_path
        self.readiness_path = readiness_path
        self.startup_path = startup_path


class HealthServer:
    """Lifecycle wrapper around the daemon-thread HTTP health server.

    Construct with a :class:`HealthConfiguration` (port/paths) and a :class:`ReadinessState`, then
    :meth:`start` it (binds and serves on a background daemon thread) and :meth:`stop` it during
    shutdown. Binding ``0.0.0.0`` on the configured port; pass ``port: 0`` in config for an ephemeral
    port (tests read the bound port via :attr:`port`).
    """

    def __init__(
        self,
        config: HealthConfiguration,
        readiness: ReadinessState,
        bind_host: str = "0.0.0.0",
    ):
        self._config = config
        self._readiness = readiness
        self._bind_host = bind_host
        self._httpd = None
        self._thread = None

    def start(self) -> None:
        """Bind the listener and start serving on a daemon thread. Raises if the port is unavailable."""
        self._httpd = _HealthHTTPServer(
            (self._bind_host, self._config.port),
            self._readiness,
            self._config.liveness_path,
            self._config.readiness_path,
            self._config.startup_path,
        )
        self._thread = threading.Thread(
            target=self._httpd.serve_forever,
            name="edgecommons-health",
            daemon=True,
        )
        self._thread.start()
        logger.info(
            "Health server listening on %s:%d (livez=%s readyz=%s startupz=%s)",
            self._bind_host,
            self.port,
            self._config.liveness_path,
            self._config.readiness_path,
            self._config.startup_path,
        )

    @property
    def port(self) -> int:
        """The actual bound port (resolves an ephemeral ``port: 0`` once started)."""
        if self._httpd is not None:
            return self._httpd.server_address[1]
        return self._config.port

    def stop(self) -> None:
        """Stop serving and release the socket. Idempotent and bounded (joins the thread briefly)."""
        if self._httpd is not None:
            try:
                self._httpd.shutdown()
                self._httpd.server_close()
            except Exception as e:  # noqa: BLE001 - shutdown must not raise
                logger.warning("Error stopping health server: %s", e)
            finally:
                self._httpd = None
        if self._thread is not None:
            self._thread.join(timeout=5.0)
            self._thread = None
