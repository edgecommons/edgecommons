//! # Metrics target — prometheus (pull-based, feature `metrics-prometheus`)
//!
//! **One-liner purpose**: Maintain an in-process Prometheus registry and serve it as
//! OpenMetrics/Prometheus text over HTTP at `/metrics`, mirroring the Java/Python/TS `prometheus`
//! target (FR-MET-1/2/3). The default metric target on the KUBERNETES platform.
//!
//! ## Inverted lifecycle (FR-MET-2)
//! Unlike every *push* target (`log`/`messaging`/`cloudwatch`/`cloudwatchcomponent`), the
//! `prometheus` target does not deliver anything on emit — **the Prometheus server scrapes it**:
//! - [`PrometheusTarget::emit`] and [`PrometheusTarget::emit_now`] only *update the in-process
//!   registry* (latest-value gauges). They are identical (there is no batching to bypass) and push
//!   nothing over the network.
//! - [`PrometheusTarget::flush`] is a **delivery no-op** — there is nothing to flush; a scrape pulls
//!   the current values. (It returns `Ok(())`.)
//! - [`PrometheusTarget::shutdown`] (and `Drop`) **stops the HTTP listener thread** so no port,
//!   thread, or task leaks.
//!
//! This inversion is local to this target; it does NOT change the [`MetricTarget`] contract for the
//! other targets, which keep their push semantics.
//!
//! ## Dimension → label mapping (FR-MET-3, locked for four-way parity)
//! For each [`Measure`](crate::metrics::metric::Measure) in an emitted metric this registers/updates
//! one [`prometheus::Gauge`] (latest-value semantics — a scrape reads the current value):
//! - **gauge name** = `sanitize(lowercase("{namespace}_{measureName}"))`, where the metric-name
//!   sanitizer replaces every char not matching `[a-z0-9_]` with `_` and prefixes `_` if the result
//!   starts with a digit (Prometheus metric-name rules). `namespace` defaults to `edgecommons`.
//! - **labels** = the metric's dimensions ([`Metric::get_dimensions`], which already include
//!   `category` (= metric name), `coreName`, `component`, plus any custom dimensions). Each label
//!   *name* is sanitized to `[a-zA-Z_][a-zA-Z0-9_]*` (invalid chars → `_`, `_`-prefixed if it starts
//!   with a digit); the label *value* is used as-is.
//! - The Greengrass `largeFleetWorkaround` (the `coreName="ALL"` duplicate) is a CloudWatch-ism with
//!   no Prometheus analog and is intentionally NOT applied here.
//!
//! Because the registry keys a gauge family by its (fully-qualified) name, the label-name *set* for a
//! given gauge name is fixed at first registration (the same constraint the Java/Python/TS Prometheus
//! clients impose). In the rare case two different metrics map to the same gauge name with a
//! *different* label-name set, the later, mismatched emit is dropped with a warning rather than
//! corrupting the family.
//!
//! ## Safety & Panics
//! No `unsafe`. The accept loop never panics: malformed requests yield 400, I/O errors are logged and
//! retried. A bounded read timeout stops a slow client from wedging the single server thread. The
//! gauge registry mutex is never held across an `.await`.
//!
//! ## Related Modules
//! - [`crate::metrics`], [`crate::metrics::metric`], [`crate::health`] (the same minimal-HTTP pattern).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use async_trait::async_trait;
use prometheus::{Encoder, GaugeVec, Opts, Registry, TextEncoder};

use super::MetricTarget;
use crate::error::{EdgeCommonsError, Result};
use crate::metrics::metric::Metric;

/// How long the accept loop sleeps between non-blocking `accept()` polls (bounds shutdown latency).
const POLL_INTERVAL: Duration = Duration::from_millis(50);
/// Read timeout for a single connection — a slow/garbage client cannot wedge the server thread.
const READ_TIMEOUT: Duration = Duration::from_secs(2);

/// A registered gauge family plus the (sorted, sanitized) label-name set it was created with. The
/// label-name set is fixed at registration; a later emit with a different set for the same gauge name
/// is rejected (see the module docs).
struct CachedGauge {
    vec: GaugeVec,
    label_names: Vec<String>,
}

/// The pull-based Prometheus metric target: an in-process registry served at an HTTP `/metrics`
/// endpoint. See the module docs for the inverted lifecycle and the dimension→label mapping.
pub struct PrometheusTarget {
    /// Metric namespace (prefix of every gauge name); defaults to `edgecommons`.
    namespace: String,
    /// The Prometheus registry (cloned into the server thread; gathered on each scrape).
    registry: Registry,
    /// Lazily-created gauge families, keyed by sanitized gauge name.
    gauges: Mutex<HashMap<String, CachedGauge>>,
    /// Set on shutdown/drop to ask the accept loop to exit.
    stop: Arc<AtomicBool>,
    /// The server thread handle, joined on shutdown/drop (whichever runs first).
    handle: Mutex<Option<JoinHandle<()>>>,
    /// The actually-bound address (resolves the ephemeral port when bound to 0 in tests).
    addr: SocketAddr,
}

impl PrometheusTarget {
    /// Bind `0.0.0.0:<port>`, spawn the `/metrics` responder thread, and return the target.
    ///
    /// `path` is the exposition route (default `/metrics`). Binding happens synchronously so
    /// [`Self::local_addr`] is valid as soon as this returns. Use `port = 0` in tests for an
    /// ephemeral port.
    ///
    /// # Errors
    /// Returns [`EdgeCommonsError::Metrics`] if the port cannot be bound (e.g. already in use) or the server
    /// thread cannot be spawned — propagated so a hot-reload rebuild keeps the previous target.
    pub fn start(namespace: impl Into<String>, port: u16, path: &str) -> Result<Self> {
        let registry = Registry::new();
        let listener = TcpListener::bind(("0.0.0.0", port)).map_err(|e| {
            EdgeCommonsError::Metrics(format!("prometheus target cannot bind port {port}: {e}"))
        })?;
        let addr = listener.local_addr().map_err(|e| {
            EdgeCommonsError::Metrics(format!("prometheus target local_addr failed: {e}"))
        })?;
        // Non-blocking accept + a short poll lets the thread observe the stop flag on shutdown
        // without an extra dependency (same pattern as the health server).
        listener.set_nonblocking(true).map_err(|e| {
            EdgeCommonsError::Metrics(format!("prometheus target set_nonblocking failed: {e}"))
        })?;

        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();
        let registry_thread = registry.clone();
        let path = path.to_string();
        let handle = std::thread::Builder::new()
            .name("edgecommons-prometheus".to_string())
            .spawn(move || serve(listener, path, registry_thread, stop_thread))
            .map_err(|e| {
                EdgeCommonsError::Metrics(format!("prometheus target thread spawn failed: {e}"))
            })?;

        tracing::info!(addr = %addr, "prometheus metric target listening");
        Ok(Self {
            namespace: namespace.into(),
            registry,
            gauges: Mutex::new(HashMap::new()),
            stop,
            handle: Mutex::new(Some(handle)),
            addr,
        })
    }

    /// The address the server is actually bound to (resolves the ephemeral port when bound to 0).
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// Stop the HTTP listener thread (idempotent): set the stop flag and join. Called by both
    /// [`MetricTarget::shutdown`] and `Drop`.
    fn stop_server(&self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Ok(mut guard) = self.handle.lock() {
            if let Some(handle) = guard.take() {
                // The accept loop notices the stop flag within POLL_INTERVAL, so this join is bounded.
                let _ = handle.join();
            }
        }
    }

    /// Update the registry for one emission (latest-value gauges). See the module docs for the
    /// dimension→label mapping. Never delivers anything over the network.
    fn record(&self, metric: &Metric, values: &HashMap<String, f64>) -> Result<()> {
        // Build the ordered, sanitized label-name + value vectors from the metric's dimensions.
        // Dimensions are a BTreeMap (deterministically sorted), so the order is stable. Drop any
        // dimension whose name collides (post-sanitization) with one already taken (keep first).
        let mut label_names: Vec<String> = Vec::new();
        let mut label_values: Vec<String> = Vec::new();
        for (key, value) in metric.get_dimensions() {
            let name = sanitize_label_name(key);
            if label_names.iter().any(|n| n == &name) {
                continue;
            }
            label_names.push(name);
            label_values.push(value.clone());
        }

        let mut gauges = self.gauges.lock().map_err(|_| {
            EdgeCommonsError::Metrics("prometheus gauge registry mutex poisoned".to_string())
        })?;

        for (measure_name, value) in values {
            let gauge_name = sanitize_metric_name(
                &format!("{}_{}", self.namespace, measure_name).to_lowercase(),
            );

            if !gauges.contains_key(&gauge_name) {
                let name_refs: Vec<&str> = label_names.iter().map(String::as_str).collect();
                let help = format!("edgecommons metric {gauge_name}");
                let vec = GaugeVec::new(Opts::new(gauge_name.clone(), help), &name_refs).map_err(
                    |e| {
                        EdgeCommonsError::Metrics(format!(
                            "prometheus gauge '{gauge_name}' construction failed: {e}"
                        ))
                    },
                )?;
                self.registry.register(Box::new(vec.clone())).map_err(|e| {
                    EdgeCommonsError::Metrics(format!(
                        "prometheus gauge '{gauge_name}' registration failed: {e}"
                    ))
                })?;
                gauges.insert(
                    gauge_name.clone(),
                    CachedGauge {
                        vec,
                        label_names: label_names.clone(),
                    },
                );
            }

            let cached = gauges
                .get(&gauge_name)
                .expect("gauge present (just inserted or pre-existing)");
            if cached.label_names != label_names {
                tracing::warn!(
                    gauge = %gauge_name,
                    "prometheus gauge already registered with a different label set; dropping this \
                     emit (a measure name maps to one stable label set — see the target docs)"
                );
                continue;
            }
            match cached.vec.get_metric_with_label_values(&label_values) {
                Ok(gauge) => gauge.set(*value),
                Err(e) => {
                    tracing::warn!(gauge = %gauge_name, error = %e, "prometheus gauge update failed")
                }
            }
        }
        Ok(())
    }
}

impl Drop for PrometheusTarget {
    fn drop(&mut self) {
        self.stop_server();
    }
}

#[async_trait]
impl MetricTarget for PrometheusTarget {
    async fn emit(&self, metric: &Metric, values: &HashMap<String, f64>) -> Result<()> {
        self.record(metric, values)
    }

    async fn emit_now(&self, metric: &Metric, values: &HashMap<String, f64>) -> Result<()> {
        // Identical to `emit`: there is no batching to bypass for a pull target.
        self.record(metric, values)
    }

    async fn flush(&self) -> Result<()> {
        // Delivery no-op (FR-MET-2): the Prometheus server pulls; there is nothing to push.
        Ok(())
    }

    async fn shutdown(&self) {
        self.stop_server();
    }
}

/// Sanitize a Prometheus *metric* name: the input is already lowercased, so keep `[a-z0-9_]`,
/// replace everything else with `_`, and prefix `_` if it starts with a digit (Prometheus
/// metric-name rules: `[a-zA-Z_][a-zA-Z0-9_]*`).
fn sanitize_metric_name(lowercased: &str) -> String {
    let mut out: String = lowercased
        .chars()
        .map(|c| {
            if matches!(c, 'a'..='z' | '0'..='9' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out
}

/// Sanitize a Prometheus *label* name: keep `[a-zA-Z0-9_]` (case preserved, so `coreName` stays
/// `coreName`), replace everything else with `_`, and prefix `_` if it starts with a digit
/// (Prometheus label-name rules: `[a-zA-Z_][a-zA-Z0-9_]*`).
fn sanitize_label_name(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| {
            if matches!(c, 'a'..='z' | 'A'..='Z' | '0'..='9' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out
}

/// The accept loop: serve connections until the stop flag is set (mirrors the health server).
fn serve(listener: TcpListener, path: String, registry: Registry, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _peer)) => handle_connection(stream, &path, &registry),
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(POLL_INTERVAL);
            }
            Err(e) => {
                tracing::debug!(error = %e, "prometheus server accept error");
                std::thread::sleep(POLL_INTERVAL);
            }
        }
    }
    tracing::debug!("prometheus server stopped");
}

/// Read one request, route it, and write the response. Best-effort: any I/O error drops the
/// connection (Prometheus retries on its scrape interval).
fn handle_connection(mut stream: TcpStream, path: &str, registry: &Registry) {
    // The listener is non-blocking; the accepted stream must be blocking for a timed read.
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(READ_TIMEOUT));

    let mut buf = [0u8; 1024];
    let n = match stream.read(&mut buf) {
        Ok(n) => n,
        Err(_) => return,
    };

    match parse_request_target(&buf[..n]) {
        Some(req) if req == path => {
            let (content_type, body) = exposition(registry);
            let _ = write_response(&mut stream, 200, "OK", &content_type, body.as_bytes());
        }
        Some(_) => {
            let _ = write_response(&mut stream, 404, "Not Found", "text/plain", b"not found");
        }
        None => {
            let _ = write_response(
                &mut stream,
                400,
                "Bad Request",
                "text/plain",
                b"bad request",
            );
        }
    }
}

/// Encode the current registry as OpenMetrics/Prometheus text via the client lib's [`TextEncoder`],
/// returning `(content_type, body)`. The encoder sets a valid `Content-Type`
/// (`text/plain; version=0.0.4`) that Prometheus 3.x accepts (it rejects a blank type).
fn exposition(registry: &Registry) -> (String, String) {
    let encoder = TextEncoder::new();
    let families = registry.gather();
    let mut buf = Vec::new();
    if let Err(e) = encoder.encode(&families, &mut buf) {
        tracing::warn!(error = %e, "prometheus exposition encode failed");
    }
    let body = String::from_utf8(buf).unwrap_or_default();
    (encoder.format_type().to_string(), body)
}

/// Extract the request-target path from the first request line (`GET /metrics?x=1 HTTP/1.1`),
/// stripping any query string. `None` if the bytes are not a parseable request line.
fn parse_request_target(raw: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(raw).ok()?;
    let first_line = text.lines().next()?;
    let mut parts = first_line.split_whitespace();
    let _method = parts.next()?; // Scrapes use GET; we route by path regardless of method.
    let target = parts.next()?;
    let path = target.split('?').next().unwrap_or(target);
    Some(path.to_string())
}

/// Write a minimal HTTP/1.1 response with `Connection: close` so the client reads to EOF.
fn write_response(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    content_type: &str,
    body: &[u8],
) -> std::io::Result<()> {
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n",
        len = body.len(),
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::metric::MetricBuilder;
    use std::time::Instant;

    fn values(measure: &str, n: f64) -> HashMap<String, f64> {
        let mut v = HashMap::new();
        v.insert(measure.to_string(), n);
        v
    }

    /// Issue a real `GET <path>` and return `(status, content_type, body)`. The server binds the
    /// wildcard `0.0.0.0` (clients can't connect to that, esp. on Windows), so dial loopback on the
    /// server's bound port.
    fn http_get(addr: SocketAddr, path: &str) -> (u16, String, String) {
        let target = SocketAddr::from(([127, 0, 0, 1], addr.port()));
        let mut stream = TcpStream::connect(target).expect("connect to prometheus server");
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
        let content_type = response
            .lines()
            .find(|l| l.to_ascii_lowercase().starts_with("content-type:"))
            .map(|l| {
                l.split_once(':')
                    .map(|x| x.1)
                    .unwrap_or("")
                    .trim()
                    .to_string()
            })
            .unwrap_or_default();
        let body = response.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
        (status, content_type, body)
    }

    // ---------- sanitization (FR-MET-3) ----------

    #[test]
    fn sanitize_metric_name_replaces_invalid_and_prefixes_digit() {
        assert_eq!(
            sanitize_metric_name("edgecommons_count"),
            "edgecommons_count"
        );
        // hostile chars (already lowercased) → '_'
        assert_eq!(sanitize_metric_name("my-ns_cpu.usage%"), "my_ns_cpu_usage_");
        // leading digit → '_' prefix
        assert_eq!(sanitize_metric_name("9lives"), "_9lives");
    }

    #[test]
    fn sanitize_label_name_preserves_case_and_fixes_invalid() {
        // case preserved (coreName stays coreName)
        assert_eq!(sanitize_label_name("coreName"), "coreName");
        assert_eq!(sanitize_label_name("my-dim.x"), "my_dim_x");
        assert_eq!(sanitize_label_name("9d"), "_9d");
    }

    // ---------- registry update + exposition over loopback ----------

    #[tokio::test]
    async fn emit_updates_registry_and_metrics_serves_openmetrics() {
        let target = PrometheusTarget::start("edgecommons", 0, "/metrics").expect("start");
        let addr = target.local_addr();

        let metric = MetricBuilder::create("requests")
            .with_thing_name("thing-1")
            .with_component_name("com.example.C")
            .add_measure("count", "Count", 60)
            .build();
        target.emit(&metric, &values("count", 5.0)).await.unwrap();

        let (status, content_type, body) = http_get(addr, "/metrics");
        assert_eq!(status, 200);
        // Prometheus 3.x rejects a blank type; the client lib sets a valid one.
        assert!(
            content_type.starts_with("text/plain"),
            "content-type was '{content_type}'"
        );
        // gauge name = sanitize(lowercase("edgecommons_count"))
        assert!(
            body.contains("edgecommons_count"),
            "body missing gauge name:\n{body}"
        );
        // dimensions became labels (category = metric name, coreName, component)
        assert!(
            body.contains("category=\"requests\""),
            "missing category label:\n{body}"
        );
        assert!(
            body.contains("coreName=\"thing-1\""),
            "missing coreName label:\n{body}"
        );
        assert!(
            body.contains("component=\"com.example.C\""),
            "missing component label:\n{body}"
        );
        // latest value
        assert!(body.contains(" 5"), "missing gauge value:\n{body}");
    }

    #[tokio::test]
    async fn emit_now_sets_latest_value() {
        let target = PrometheusTarget::start("ns", 0, "/metrics").expect("start");
        let addr = target.local_addr();
        let metric = MetricBuilder::create("m")
            .add_measure("v", "None", 60)
            .build();

        target.emit(&metric, &values("v", 1.0)).await.unwrap();
        target.emit_now(&metric, &values("v", 42.0)).await.unwrap();

        let (_, _, body) = http_get(addr, "/metrics");
        // latest-value gauge semantics: the last write wins.
        let line = body
            .lines()
            .find(|l| l.starts_with("ns_v{") || l.starts_with("ns_v "))
            .unwrap_or("");
        assert!(
            line.contains("42"),
            "expected latest value 42, body:\n{body}"
        );
    }

    #[tokio::test]
    async fn flush_is_a_delivery_no_op() {
        // flush() must not error and must not deliver anything (a pull target has nothing to push).
        let target = PrometheusTarget::start("ns", 0, "/metrics").expect("start");
        let metric = MetricBuilder::create("m")
            .add_measure("v", "None", 60)
            .build();
        target.emit(&metric, &values("v", 1.0)).await.unwrap();
        target.flush().await.unwrap();
        // The value is still only visible via a scrape, not pushed anywhere.
        let (status, _, body) = http_get(target.local_addr(), "/metrics");
        assert_eq!(status, 200);
        assert!(body.contains("ns_v"));
    }

    #[tokio::test]
    async fn non_metrics_path_is_404() {
        let target = PrometheusTarget::start("ns", 0, "/metrics").expect("start");
        let (status, _, _) = http_get(target.local_addr(), "/nope");
        assert_eq!(status, 404);
    }

    #[tokio::test]
    async fn custom_path_is_served() {
        let target = PrometheusTarget::start("ns", 0, "/prom").expect("start");
        let metric = MetricBuilder::create("m")
            .add_measure("v", "None", 60)
            .build();
        target.emit(&metric, &values("v", 7.0)).await.unwrap();
        assert_eq!(http_get(target.local_addr(), "/prom").0, 200);
        assert_eq!(
            http_get(target.local_addr(), "/metrics").0,
            404,
            "default path off when remapped"
        );
    }

    #[tokio::test]
    async fn hostile_dimension_names_are_sanitized_in_labels() {
        let metric = MetricBuilder::create("m")
            .add_dimension("bad-dim.name", "v1")
            .add_dimension("9starts", "v2")
            .add_measure("v", "None", 60)
            .build();
        let target = PrometheusTarget::start("ns", 0, "/metrics").expect("start");
        target.emit(&metric, &values("v", 1.0)).await.unwrap();

        let (_, _, body) = http_get(target.local_addr(), "/metrics");
        assert!(
            body.contains("bad_dim_name=\"v1\""),
            "hostile dim not sanitized:\n{body}"
        );
        assert!(
            body.contains("_9starts=\"v2\""),
            "leading-digit dim not sanitized:\n{body}"
        );
    }

    #[tokio::test]
    async fn shutdown_releases_the_port() {
        let target = PrometheusTarget::start("ns", 0, "/metrics").expect("start");
        let addr = target.local_addr();
        assert_eq!(http_get(addr, "/metrics").0, 200);

        target.shutdown().await;

        // After shutdown the listener is closed; a fresh bind on the same port must eventually
        // succeed, proving the server thread released the socket (bounded by the poll interval).
        let mut bound = false;
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if TcpListener::bind(addr).is_ok() {
                bound = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(
            bound,
            "prometheus server did not release its port after shutdown"
        );
    }

    #[tokio::test]
    async fn drop_stops_the_listener() {
        let addr = {
            let target = PrometheusTarget::start("ns", 0, "/metrics").expect("start");
            let addr = target.local_addr();
            assert_eq!(http_get(addr, "/metrics").0, 200);
            addr
            // target dropped here
        };
        let mut bound = false;
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if TcpListener::bind(addr).is_ok() {
                bound = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(bound, "prometheus server did not release its port on drop");
    }
}
