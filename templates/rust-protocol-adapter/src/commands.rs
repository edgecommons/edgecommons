//! # The southbound command surface — the `sb/*` verbs + the three edge-console panels
//!
//! This module owns the whole `gg.commands()` registration for `<<COMPONENTNAME>>`: `sb/status`,
//! `sb/read`, `sb/write`, `sb/signals`, `sb/browse`, `sb/pause`, `sb/resume`, `reconnect`, `repoll`.
//! It is the generic southbound command family (SOUTHBOUND.md §2.2) every adapter serves — a real
//! adapter changes the *seam* behind it, not this surface.
//!
//! ## Conventions every verb follows
//!
//! * **Instance routing (D-EIP-13):** `body.instance` is optional iff exactly one device is
//!   configured; with two or more, a missing id is `BAD_ARGS` and an unknown id is `NO_SUCH_INSTANCE`.
//! * **Standardized error codes:** `BAD_ARGS`, `NO_SUCH_INSTANCE`, `WRITE_NOT_ALLOWED`,
//!   `WRITE_FAILED`, `DEVICE_UNAVAILABLE`, `READ_FAILED`, `RECONNECT_FAILED`, `BROWSE_UNSUPPORTED`,
//!   `BROWSE_FAILED`.
//! * **The session is never touched here.** Every verb that reads/writes/reconnects/pauses is sent
//!   to the device's own task as a [`DeviceControl`] and *confirmed* through the reply that rides it,
//!   because the session lives in that task and is not `Sync`.
//! * **`sb/write` allow-lists BEFORE any device I/O.** A refused entry never becomes a
//!   [`DeviceControl::Write`] — an adapter that writes whatever it is asked to is a control-system
//!   vulnerability, not a feature.
//! * Every verb records into the `<<COMPONENTNAME>>Command` metric family (`instance`×`verb`×`result`).
//!
//! Three panels (`overview`, `signals`, `diagnostics`) are registered via `commands.register_panel`
//! for the edge-console descriptor surface — each `scope: "instance"`, `order` 10/20/30.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use edgecommons::prelude::{command_handler, CommandError, CommandInbox};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};

use crate::app::{DeviceConfig, DeviceControl, Health, LinkState, WriteRequest};
use crate::device::{BrowseError, Quality, Reading, SignalInfo};
use crate::metrics::DeviceMetrics;

/// The per-device handles the command surface routes on: the config (routing, allow-list, inventory),
/// the control channel (session-touching verbs), the shared health (status/paused), and the metrics
/// emitter (per-verb command counters).
pub struct DeviceHandle {
    pub cfg: DeviceConfig,
    pub control: mpsc::Sender<DeviceControl>,
    pub health: Arc<Health>,
    pub dm: Arc<DeviceMetrics>,
    /// The signal inventory `sb/signals` returns — a config/backend view, no device round-trip.
    pub signals: Vec<SignalInfo>,
}

/// Register every `sb/*` verb + the three edge-console panels on the inbox.
///
/// # Errors
/// Propagates [`CommandInbox::register`] / [`CommandInbox::register_panel`] failures (a verb/panel
/// name clash or an invalid token).
pub fn register_all(commands: &CommandInbox, handles: Vec<DeviceHandle>) -> anyhow::Result<()> {
    let commander = Arc::new(Commander::new(handles));

    macro_rules! verb {
        ($name:expr, $method:ident) => {{
            let c = Arc::clone(&commander);
            commands.register(
                $name,
                command_handler(move |req| {
                    let c = Arc::clone(&c);
                    async move { c.$method(&req.body).await }
                }),
            )?;
        }};
    }

    verb!("sb/status", status);
    verb!("sb/read", read);
    verb!("sb/write", write);
    verb!("sb/signals", signals);
    verb!("sb/browse", browse);
    verb!("sb/resume", resume);
    verb!("reconnect", reconnect);
    verb!("repoll", repoll);

    // `sb/pause` additionally carries the requester identity path (the `by` field of the event).
    {
        let c = Arc::clone(&commander);
        commands.register(
            "sb/pause",
            command_handler(move |req| {
                let c = Arc::clone(&c);
                async move {
                    let by = req.identity.as_ref().map(|i| i.path().to_string());
                    c.pause(&req.body, by).await
                }
            }),
        )?;
    }

    for panel in panels() {
        commands.register_panel(panel)?;
    }
    Ok(())
}

/// The three edge-console panel descriptors. Core validates `id`/`title`/uniqueness; the widget kinds
/// and bound verbs are console-interpreted, so they ride verbatim. `order` 10/20/30, `scope: "instance"`.
#[must_use]
pub fn panels() -> Vec<Value> {
    vec![
        json!({
            "id": "overview", "title": "Overview", "order": 10, "scope": "instance",
            "widgets": [
                { "kind": "summary", "fields": ["connected", "state", "paused", "endpoint"] },
                { "kind": "commandSummary", "actions": ["reconnect", "sb/pause", "sb/resume"] }
            ],
            "verbs": ["sb/status", "reconnect", "sb/pause", "sb/resume"]
        }),
        json!({
            "id": "signals", "title": "Signals", "order": 20, "scope": "instance",
            "widgets": [ { "kind": "signalGrid" } ],
            "verbs": ["sb/signals", "sb/read", "sb/write", "repoll"]
        }),
        json!({
            "id": "diagnostics", "title": "Diagnostics", "order": 30, "scope": "instance",
            "widgets": [ { "kind": "treeBrowser" }, { "kind": "keyValueList" } ],
            "verbs": ["sb/browse", "sb/status"]
        }),
    ]
}

/// The command dispatcher: owns the per-device handles + the config order (for the single-instance
/// default).
struct Commander {
    devices: HashMap<String, DeviceHandle>,
    ids: Vec<String>,
}

type Reply = std::result::Result<Option<Value>, CommandError>;

impl Commander {
    fn new(handles: Vec<DeviceHandle>) -> Self {
        let ids: Vec<String> = handles.iter().map(|h| h.cfg.id.clone()).collect();
        let devices = handles.into_iter().map(|h| (h.cfg.id.clone(), h)).collect();
        Self { devices, ids }
    }

    /// Route to the addressed device (D-EIP-13): `body.instance` optional iff exactly one device is
    /// configured; with two or more a missing/unknown id is `BAD_ARGS` / `NO_SUCH_INSTANCE`.
    fn resolve(&self, body: &Value) -> std::result::Result<&DeviceHandle, CommandError> {
        match body.get("instance").and_then(Value::as_str) {
            Some(id) => self
                .devices
                .get(id)
                .ok_or_else(|| CommandError::new("NO_SUCH_INSTANCE", format!("no configured device `{id}`"))),
            None => {
                if self.ids.len() == 1 {
                    Ok(self.devices.get(&self.ids[0]).expect("one device"))
                } else {
                    Err(CommandError::new(
                        "BAD_ARGS",
                        "field `instance` is required when multiple devices are configured",
                    ))
                }
            }
        }
    }

    // --- sb/status ---------------------------------------------------------------------------------

    async fn status(&self, body: &Value) -> Reply {
        let h = self.resolve(body)?;
        let started = Instant::now();
        let link = h.health.link();
        let connected = link == LinkState::Online;
        let paused = h.health.is_paused();
        let state = if paused && connected { "PAUSED" } else { link.as_str() };
        let out = json!({
            "id": h.cfg.id,
            "adapter": h.cfg.adapter,
            "connected": connected,
            "state": state,
            "paused": paused,
            "endpoint": h.cfg.connection.endpoint,
            "metrics": h.dm.counters_view(),
        });
        h.dm.record_command("sb/status", true, ms(started));
        Ok(Some(out))
    }

    // --- sb/signals (the configured inventory, no device I/O) --------------------------------------

    async fn signals(&self, body: &Value) -> Reply {
        let h = self.resolve(body)?;
        let started = Instant::now();
        let signals: Vec<Value> = h
            .signals
            .iter()
            .map(|s| {
                json!({
                    "id": s.id,
                    "name": s.name,
                    "writable": h.cfg.writes.permits(&s.id),
                })
            })
            .collect();
        h.dm.record_command("sb/signals", true, ms(started));
        Ok(Some(json!({ "id": h.cfg.id, "signals": signals })))
    }

    // --- sb/read (on-demand read of named signals) ------------------------------------------------

    async fn read(&self, body: &Value) -> Reply {
        let h = self.resolve(body)?;
        let started = Instant::now();
        let refs = body
            .get("signals")
            .and_then(Value::as_array)
            .ok_or_else(|| CommandError::new("BAD_ARGS", "expected a `signals` array"))?;

        // Resolve each ref to a stable id (keeping the request order for the reply).
        let plan: Vec<std::result::Result<String, String>> =
            refs.iter().map(|r| self.resolve_ref(h, r)).collect();
        let ids: Vec<String> = plan.iter().filter_map(|r| r.clone().ok()).collect();

        let readings: HashMap<String, Reading> = if ids.is_empty() {
            HashMap::new()
        } else {
            let (tx, rx) = oneshot::channel();
            h.control
                .send(DeviceControl::ReadNow { ids, reply: tx })
                .await
                .map_err(|_| device_unavailable())?;
            match rx.await {
                Ok(Ok(rs)) => rs.into_iter().map(|r| (r.signal_id.clone(), r)).collect(),
                Ok(Err(e)) => {
                    h.dm.record_command("sb/read", false, ms(started));
                    return Err(CommandError::new("READ_FAILED", e));
                }
                Err(_) => {
                    h.dm.record_command("sb/read", false, ms(started));
                    return Err(device_unavailable());
                }
            }
        };

        let reads: Vec<Value> = plan
            .into_iter()
            .map(|entry| match entry {
                Ok(id) => match readings.get(&id) {
                    Some(r) => json!({
                        "signal": { "id": id },
                        "value": r.value,
                        "quality": quality_str(r.quality),
                        "qualityRaw": r.quality_raw,
                    }),
                    None => bad_read(&id, "NO_DATA"),
                },
                Err(label) => bad_read(&label, "UNRESOLVED_REF"),
            })
            .collect();

        h.dm.record_command("sb/read", true, ms(started));
        Ok(Some(json!({ "id": h.cfg.id, "reads": reads })))
    }

    // --- sb/write (§2.2 batch shape; allow-list BEFORE any device I/O; confirmed) ------------------

    async fn write(&self, body: &Value) -> Reply {
        let h = self.resolve(body)?;
        let started = Instant::now();
        let entries = write_entries(body)?;

        let mut results = Vec::with_capacity(entries.len());
        let mut refused = 0usize;
        let mut attempted = 0usize;
        let mut succeeded = 0usize;

        for entry in &entries {
            let id = match self.resolve_ref(h, entry) {
                Ok(id) => id,
                Err(label) => {
                    results.push(json!({ "signal": label, "ok": false, "error": "unresolved ref" }));
                    continue;
                }
            };
            // THE ALLOW-LIST — checked here, BEFORE the write ever reaches the device.
            if !h.cfg.writes.permits(&id) {
                refused += 1;
                results.push(json!({ "signal": id, "ok": false, "error": "not in writes.allow" }));
                continue;
            }
            let Some(value) = entry.get("value").cloned() else {
                results.push(json!({ "signal": id, "ok": false, "error": "missing value" }));
                continue;
            };

            attempted += 1;
            let (tx, rx) = oneshot::channel();
            h.control
                .send(DeviceControl::Write(WriteRequest { signal_id: id.clone(), value: value.clone(), ack: tx }))
                .await
                .map_err(|_| device_unavailable())?;
            match rx.await {
                Ok(Ok(())) => {
                    succeeded += 1;
                    results.push(json!({ "signal": id, "value": value, "ok": true }));
                }
                Ok(Err(e)) => results.push(json!({ "signal": id, "value": value, "ok": false, "error": e })),
                Err(_) => return Err(device_unavailable()),
            }
        }

        // WRITE_NOT_ALLOWED only when EVERY entry was an allow-list refusal (nothing else attempted).
        if !entries.is_empty() && refused == entries.len() {
            h.dm.record_command("sb/write", false, ms(started));
            return Err(CommandError::new("WRITE_NOT_ALLOWED", "no entry is in this instance's writes.allow list"));
        }
        // WRITE_FAILED when every allowed write reached the device and every one failed.
        if attempted > 0 && succeeded == 0 {
            h.dm.record_command("sb/write", false, ms(started));
            return Err(CommandError::new("WRITE_FAILED", "every attempted write was rejected by the device"));
        }

        h.dm.record_command("sb/write", true, ms(started));
        Ok(Some(json!({ "id": h.cfg.id, "written": succeeded, "results": results })))
    }

    // --- sb/browse (paged address-space discovery) ------------------------------------------------

    async fn browse(&self, body: &Value) -> Reply {
        let h = self.resolve(body)?;
        let started = Instant::now();
        let cursor = body.get("cursor").and_then(Value::as_str).map(str::to_string);
        let max = body.get("max").and_then(Value::as_u64).unwrap_or(200).clamp(1, 1000) as usize;

        let (tx, rx) = oneshot::channel();
        h.control
            .send(DeviceControl::Browse { cursor, max, reply: tx })
            .await
            .map_err(|_| device_unavailable())?;
        let result = match rx.await {
            Ok(Ok(page)) => {
                let entries: Vec<Value> = page
                    .entries
                    .iter()
                    .map(|e| json!({ "id": e.id, "name": e.name, "type": e.type_name }))
                    .collect();
                let mut out = json!({ "id": h.cfg.id, "entries": entries });
                if let Some(cursor) = page.next_cursor {
                    out["cursor"] = json!(cursor);
                }
                Ok(Some(out))
            }
            Ok(Err(BrowseError::Unsupported)) => {
                Err(CommandError::new("BROWSE_UNSUPPORTED", "this adapter has no discovery service"))
            }
            Ok(Err(BrowseError::Failed(e))) => Err(CommandError::new("BROWSE_FAILED", e)),
            Err(_) => Err(device_unavailable()),
        };
        h.dm.record_command("sb/browse", result.is_ok(), ms(started));
        result
    }

    // --- sb/pause + sb/resume (idempotent {paused, changed}) --------------------------------------

    async fn pause(&self, body: &Value, _by: Option<String>) -> Reply {
        let h = self.resolve(body)?;
        let started = Instant::now();
        let (tx, rx) = oneshot::channel();
        h.control
            .send(DeviceControl::Pause { reply: tx })
            .await
            .map_err(|_| device_unavailable())?;
        let changed = rx.await.map_err(|_| device_unavailable())?;
        h.dm.record_command("sb/pause", true, ms(started));
        Ok(Some(json!({ "id": h.cfg.id, "paused": true, "changed": changed })))
    }

    async fn resume(&self, body: &Value) -> Reply {
        let h = self.resolve(body)?;
        let started = Instant::now();
        let (tx, rx) = oneshot::channel();
        h.control
            .send(DeviceControl::Resume { reply: tx })
            .await
            .map_err(|_| device_unavailable())?;
        let changed = rx.await.map_err(|_| device_unavailable())?;
        h.dm.record_command("sb/resume", true, ms(started));
        Ok(Some(json!({ "id": h.cfg.id, "paused": false, "changed": changed })))
    }

    // --- reconnect ---------------------------------------------------------------------------------

    async fn reconnect(&self, body: &Value) -> Reply {
        let h = self.resolve(body)?;
        let started = Instant::now();
        let (tx, rx) = oneshot::channel();
        h.control
            .send(DeviceControl::Reconnect { reply: tx })
            .await
            .map_err(|_| device_unavailable())?;
        match rx.await.map_err(|_| device_unavailable())? {
            Ok(()) => {
                h.dm.record_command("reconnect", true, ms(started));
                Ok(Some(json!({ "id": h.cfg.id, "connected": true })))
            }
            Err(e) => {
                h.dm.record_command("reconnect", false, ms(started));
                Err(CommandError::new("RECONNECT_FAILED", e))
            }
        }
    }

    // --- repoll (refused while paused) ------------------------------------------------------------

    async fn repoll(&self, body: &Value) -> Reply {
        let h = self.resolve(body)?;
        let started = Instant::now();
        if h.health.is_paused() {
            h.dm.record_command("repoll", false, ms(started));
            return Err(CommandError::new("BAD_ARGS", "instance is paused - resume first"));
        }
        let (tx, rx) = oneshot::channel();
        h.control
            .send(DeviceControl::Repoll { reply: tx })
            .await
            .map_err(|_| device_unavailable())?;
        match rx.await.map_err(|_| device_unavailable())? {
            Ok(polled) => {
                h.dm.record_command("repoll", true, ms(started));
                Ok(Some(json!({ "id": h.cfg.id, "polled": polled })))
            }
            Err(e) if e.contains("paused") => {
                h.dm.record_command("repoll", false, ms(started));
                Err(CommandError::new("BAD_ARGS", e))
            }
            Err(e) => {
                h.dm.record_command("repoll", false, ms(started));
                Err(CommandError::new("DEVICE_UNAVAILABLE", e))
            }
        }
    }

    /// Resolve a `sb/read`/`sb/write` signal-ref to its stable id: `{"signalId"}` / `{"id"}` directly,
    /// or `{"name"}` looked up against the configured inventory. `Err` carries a label for the BAD /
    /// unresolved entry.
    fn resolve_ref(&self, h: &DeviceHandle, r: &Value) -> std::result::Result<String, String> {
        if let Some(id) = r.get("signalId").and_then(Value::as_str) {
            return Ok(id.to_string());
        }
        if let Some(id) = r.get("id").and_then(Value::as_str) {
            return Ok(id.to_string());
        }
        if let Some(name) = r.get("name").and_then(Value::as_str) {
            return h
                .signals
                .iter()
                .find(|s| s.name.as_deref() == Some(name))
                .map(|s| s.id.clone())
                .ok_or_else(|| name.to_string());
        }
        Err("<invalid ref>".to_string())
    }
}

// =================================================================================================
// Helpers
// =================================================================================================

fn ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn device_unavailable() -> CommandError {
    CommandError::new("DEVICE_UNAVAILABLE", "device task is unavailable")
}

fn quality_str(q: Quality) -> &'static str {
    match q {
        Quality::Good => "GOOD",
        Quality::Bad => "BAD",
        Quality::Uncertain => "UNCERTAIN",
    }
}

fn bad_read(id: &str, raw: &str) -> Value {
    json!({ "signal": { "id": id }, "value": Value::Null, "quality": "BAD", "qualityRaw": raw })
}

/// Normalize an `sb/write` body to a list of `{ref…, value}` entries: a `writes` array, or a single
/// object carrying `value` (§2.2). `Err(BAD_ARGS)` when neither form is present.
fn write_entries(body: &Value) -> std::result::Result<Vec<Value>, CommandError> {
    if let Some(arr) = body.get("writes").and_then(Value::as_array) {
        return Ok(arr.clone());
    }
    if body.get("value").is_some() {
        return Ok(vec![body.clone()]);
    }
    Err(CommandError::new("BAD_ARGS", "expected a `writes` array or a single write object with `value`"))
}

#[cfg(test)]
mod tests {
    //! Every verb's happy path + each error code + the single-instance default; the allow-list
    //! refusal proven to happen BEFORE any device I/O; pause gating a poll; and the panel registration.
    //! A mock device task services the control channel and RECORDS every write that reaches it — no
    //! device, no socket.
    use super::*;
    use std::sync::Mutex;

    use edgecommons::prelude::{Config, Metric, MetricService};

    use crate::app::{set_paused, Health};
    use crate::device::{BrowsePage, BrowsedSignal};

    // --- a no-op MetricService + Config so DeviceMetrics can be built without a live runtime --------

    #[derive(Default)]
    struct NoopMetrics;

    #[async_trait::async_trait]
    impl MetricService for NoopMetrics {
        fn define_metric(&self, _metric: Metric) {}
        fn is_metric_defined(&self, _name: &str) -> bool {
            true
        }
        async fn emit_metric(&self, _name: &str, _values: HashMap<String, f64>) -> edgecommons::Result<()> {
            Ok(())
        }
        async fn emit_metric_now(&self, _name: &str, _values: HashMap<String, f64>) -> edgecommons::Result<()> {
            Ok(())
        }
        async fn flush_metrics(&self) -> edgecommons::Result<()> {
            Ok(())
        }
        async fn shutdown(&self) {}
    }

    fn config() -> Arc<Config> {
        Arc::new(
            Config::from_value(
                "com.example.MyAdapter",
                "thing-1",
                json!({ "metricEmission": { "target": "log", "namespace": "test" } }),
            )
            .unwrap(),
        )
    }

    fn dev(v: Value) -> DeviceConfig {
        serde_json::from_value(v).unwrap()
    }

    fn a_device() -> DeviceConfig {
        dev(json!({
            "id": "plc-1",
            "adapter": "sim",
            "connection": { "endpoint": "sim://plc-1" },
            "writes": { "allow": ["setpoint-1"] }
        }))
    }

    fn sim_signals() -> Vec<SignalInfo> {
        vec![
            SignalInfo { id: "temperature-1".into(), name: Some("Ambient temperature".into()) },
            SignalInfo { id: "setpoint-1".into(), name: Some("Setpoint".into()) },
        ]
    }

    #[derive(Clone)]
    enum BrowseKind {
        One,
        Unsupported,
        Failed,
    }

    #[derive(Clone)]
    struct MockOpts {
        write_ok: bool,
        read_ok: bool,
        reconnect_ok: bool,
        repoll_ok: bool,
        browse: BrowseKind,
    }

    impl Default for MockOpts {
        fn default() -> Self {
            Self { write_ok: true, read_ok: true, reconnect_ok: true, repoll_ok: true, browse: BrowseKind::One }
        }
    }

    struct Harness {
        commander: Arc<Commander>,
        /// Every write that REACHED the device — empty proves the allow-list refused before any I/O.
        writes: Arc<Mutex<Vec<(String, Value)>>>,
        health: Arc<Health>,
        _task: tokio::task::JoinHandle<()>,
    }

    fn make_dm(cfg: &DeviceConfig, health: Arc<Health>) -> Arc<DeviceMetrics> {
        Arc::new(DeviceMetrics::new(Arc::new(NoopMetrics), config(), cfg.id.clone(), health, 30))
    }

    /// Build a single-device commander whose control channel is served by a mock device task.
    fn harness(cfg: DeviceConfig, opts: MockOpts) -> Harness {
        let (tx, mut rx) = mpsc::channel::<DeviceControl>(16);
        let health = Arc::new(Health::default());
        health.set_link(LinkState::Online);
        let dm = make_dm(&cfg, Arc::clone(&health));
        let writes = Arc::new(Mutex::new(Vec::new()));

        let t_health = Arc::clone(&health);
        let t_writes = Arc::clone(&writes);
        let task = tokio::spawn(async move {
            while let Some(ctrl) = rx.recv().await {
                match ctrl {
                    DeviceControl::Write(req) => {
                        t_writes.lock().unwrap().push((req.signal_id.clone(), req.value.clone()));
                        let _ = req.ack.send(if opts.write_ok { Ok(()) } else { Err("device rejected".into()) });
                    }
                    DeviceControl::ReadNow { ids, reply } => {
                        if opts.read_ok {
                            let rs = ids
                                .iter()
                                .map(|id| Reading {
                                    signal_id: id.clone(),
                                    name: None,
                                    value: json!(42.0),
                                    quality: Quality::Good,
                                    quality_raw: Some("OK".into()),
                                })
                                .collect();
                            let _ = reply.send(Ok(rs));
                        } else {
                            let _ = reply.send(Err("link error".into()));
                        }
                    }
                    DeviceControl::Browse { reply, .. } => {
                        let r = match opts.browse {
                            BrowseKind::One => Ok(BrowsePage {
                                entries: vec![BrowsedSignal {
                                    id: "temperature-1".into(),
                                    name: Some("Ambient temperature".into()),
                                    type_name: "REAL".into(),
                                }],
                                next_cursor: None,
                            }),
                            BrowseKind::Unsupported => Err(BrowseError::Unsupported),
                            BrowseKind::Failed => Err(BrowseError::Failed("mid-browse error".into())),
                        };
                        let _ = reply.send(r);
                    }
                    DeviceControl::Pause { reply } => {
                        let _ = reply.send(set_paused(&t_health, true));
                    }
                    DeviceControl::Resume { reply } => {
                        let _ = reply.send(set_paused(&t_health, false));
                    }
                    DeviceControl::Reconnect { reply } => {
                        let _ = reply.send(if opts.reconnect_ok { Ok(()) } else { Err("no route to host".into()) });
                    }
                    DeviceControl::Repoll { reply } => {
                        let _ = reply.send(if opts.repoll_ok { Ok(2) } else { Err("link error".into()) });
                    }
                }
            }
        });

        let handle = DeviceHandle { cfg, control: tx, health: Arc::clone(&health), dm, signals: sim_signals() };
        let commander = Arc::new(Commander::new(vec![handle]));
        Harness { commander, writes, health, _task: task }
    }

    fn ok(reply: Reply) -> Value {
        reply.expect("command succeeded").expect("a result object")
    }
    fn err_code(reply: Reply) -> String {
        reply.expect_err("command failed").code
    }

    // --- routing / single-instance default (D-EIP-13) ---------------------------------------------

    #[tokio::test]
    async fn instance_defaults_to_the_sole_device_and_unknown_or_missing_ids_error() {
        let h = harness(a_device(), MockOpts::default());
        let out = ok(h.commander.status(&json!({})).await);
        assert_eq!(out["id"], json!("plc-1"));
        assert_eq!(err_code(h.commander.status(&json!({ "instance": "nope" })).await), "NO_SUCH_INSTANCE");

        // Two devices: a missing `instance` is BAD_ARGS.
        let mk = |cfg: DeviceConfig| {
            let (tx, _rx) = mpsc::channel(1);
            let health = Arc::new(Health::default());
            let dm = make_dm(&cfg, Arc::clone(&health));
            DeviceHandle { cfg, control: tx, health, dm, signals: sim_signals() }
        };
        let mut b = a_device();
        b.id = "plc-2".into();
        let multi = Commander::new(vec![mk(a_device()), mk(b)]);
        assert_eq!(err_code(multi.status(&json!({})).await), "BAD_ARGS");
        assert_eq!(ok(multi.status(&json!({ "instance": "plc-2" })).await)["id"], json!("plc-2"));
    }

    // --- sb/status ---------------------------------------------------------------------------------

    #[tokio::test]
    async fn status_reports_connected_state_paused_and_a_counter_snapshot() {
        let h = harness(a_device(), MockOpts::default());
        let out = ok(h.commander.status(&json!({})).await);
        assert_eq!(out["connected"], json!(true));
        assert_eq!(out["state"], json!("ONLINE"));
        assert_eq!(out["paused"], json!(false));
        assert_eq!(out["adapter"], json!("sim"));
        assert!(out["metrics"].get("connectAttempts").is_some());
    }

    // --- sb/signals --------------------------------------------------------------------------------

    #[tokio::test]
    async fn signals_lists_the_inventory_with_the_writable_flag() {
        let h = harness(a_device(), MockOpts::default());
        let out = ok(h.commander.signals(&json!({})).await);
        let sigs = out["signals"].as_array().unwrap();
        assert_eq!(sigs.len(), 2);
        let setpoint = sigs.iter().find(|s| s["id"] == json!("setpoint-1")).unwrap();
        assert_eq!(setpoint["writable"], json!(true), "setpoint-1 is on the allow-list");
        let temp = sigs.iter().find(|s| s["id"] == json!("temperature-1")).unwrap();
        assert_eq!(temp["writable"], json!(false), "temperature-1 is not");
    }

    // --- sb/read -----------------------------------------------------------------------------------

    #[tokio::test]
    async fn read_returns_values_by_id_and_by_name_and_marks_unresolved_refs() {
        let h = harness(a_device(), MockOpts::default());
        let out = ok(h
            .commander
            .read(&json!({ "signals": [ { "signalId": "temperature-1" }, { "name": "Setpoint" }, { "name": "ghost" } ] }))
            .await);
        let reads = out["reads"].as_array().unwrap();
        assert_eq!(reads[0]["signal"]["id"], json!("temperature-1"));
        assert_eq!(reads[0]["quality"], json!("GOOD"));
        assert_eq!(reads[1]["signal"]["id"], json!("setpoint-1"), "resolved by name");
        assert_eq!(reads[2]["quality"], json!("BAD"), "an unknown name is a BAD/unresolved entry");
        assert_eq!(reads[2]["qualityRaw"], json!("UNRESOLVED_REF"));
    }

    #[tokio::test]
    async fn read_without_a_signals_array_is_bad_args_and_a_link_error_is_read_failed() {
        let h = harness(a_device(), MockOpts::default());
        assert_eq!(err_code(h.commander.read(&json!({})).await), "BAD_ARGS");

        let h = harness(a_device(), MockOpts { read_ok: false, ..MockOpts::default() });
        assert_eq!(
            err_code(h.commander.read(&json!({ "signals": [ { "signalId": "temperature-1" } ] })).await),
            "READ_FAILED"
        );
    }

    // --- sb/write: allow-list BEFORE any device I/O (the security guarantee) -----------------------

    #[tokio::test]
    async fn a_refused_write_never_reaches_the_device() {
        let h = harness(a_device(), MockOpts::default());
        // temperature-1 is NOT on the allow-list.
        let code = err_code(
            h.commander
                .write(&json!({ "writes": [ { "signalId": "temperature-1", "value": 1 } ] }))
                .await,
        );
        assert_eq!(code, "WRITE_NOT_ALLOWED");
        assert!(h.writes.lock().unwrap().is_empty(), "the refused write must never reach the device");
    }

    #[tokio::test]
    async fn an_allow_listed_write_is_confirmed_and_batches_mix_results() {
        let h = harness(a_device(), MockOpts::default());
        // A single allowed write (single-object shorthand).
        let out = ok(h.commander.write(&json!({ "signalId": "setpoint-1", "value": 42 })).await);
        assert_eq!(out["written"], json!(1));
        assert_eq!(h.writes.lock().unwrap().len(), 1, "the allowed write reached the device");

        // A batch: one allowed (written), one refused (never sent).
        let out = ok(h
            .commander
            .write(&json!({ "writes": [
                { "signalId": "setpoint-1", "value": 7 },
                { "signalId": "temperature-1", "value": 8 }
            ] }))
            .await);
        assert_eq!(out["written"], json!(1), "only the allow-listed entry is written");
        let results = out["results"].as_array().unwrap();
        assert_eq!(results.iter().filter(|r| r["ok"] == json!(true)).count(), 1);
        assert_eq!(results.iter().filter(|r| r["error"] == json!("not in writes.allow")).count(), 1);
        // Two device writes total (one from each successful call); the refused entry added none.
        assert_eq!(h.writes.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_write_the_device_rejects_is_write_failed() {
        let h = harness(a_device(), MockOpts { write_ok: false, ..MockOpts::default() });
        let code = err_code(h.commander.write(&json!({ "signalId": "setpoint-1", "value": 42 })).await);
        assert_eq!(code, "WRITE_FAILED");
    }

    #[tokio::test]
    async fn a_write_with_no_writes_or_value_is_bad_args() {
        let h = harness(a_device(), MockOpts::default());
        assert_eq!(err_code(h.commander.write(&json!({})).await), "BAD_ARGS");
    }

    // --- sb/browse ---------------------------------------------------------------------------------

    #[tokio::test]
    async fn browse_returns_a_page_or_the_right_error_code() {
        let h = harness(a_device(), MockOpts::default());
        let out = ok(h.commander.browse(&json!({})).await);
        assert_eq!(out["entries"].as_array().unwrap().len(), 1);
        assert_eq!(out["entries"][0]["id"], json!("temperature-1"));

        let h = harness(a_device(), MockOpts { browse: BrowseKind::Unsupported, ..MockOpts::default() });
        assert_eq!(err_code(h.commander.browse(&json!({})).await), "BROWSE_UNSUPPORTED");

        let h = harness(a_device(), MockOpts { browse: BrowseKind::Failed, ..MockOpts::default() });
        assert_eq!(err_code(h.commander.browse(&json!({})).await), "BROWSE_FAILED");
    }

    // --- pause / resume / repoll -------------------------------------------------------------------

    #[tokio::test]
    async fn pause_is_idempotent_and_repoll_is_refused_while_paused() {
        let h = harness(a_device(), MockOpts::default());

        // repoll works while running.
        assert_eq!(ok(h.commander.repoll(&json!({})).await)["polled"], json!(2));

        let out = ok(h.commander.pause(&json!({}), None).await);
        assert_eq!(out["paused"], json!(true));
        assert_eq!(out["changed"], json!(true));
        assert!(h.health.is_paused());

        // repoll is refused while paused (BAD_ARGS).
        assert_eq!(err_code(h.commander.repoll(&json!({})).await), "BAD_ARGS");

        // pausing again is idempotent.
        assert_eq!(ok(h.commander.pause(&json!({}), None).await)["changed"], json!(false));

        // resume clears it and repoll works again.
        let out = ok(h.commander.resume(&json!({})).await);
        assert_eq!(out["paused"], json!(false));
        assert_eq!(out["changed"], json!(true));
        assert!(!h.health.is_paused());
        assert_eq!(ok(h.commander.repoll(&json!({})).await)["polled"], json!(2));
    }

    // --- reconnect ---------------------------------------------------------------------------------

    #[tokio::test]
    async fn reconnect_confirms_or_reports_reconnect_failed() {
        let h = harness(a_device(), MockOpts::default());
        assert_eq!(ok(h.commander.reconnect(&json!({})).await)["connected"], json!(true));

        let h = harness(a_device(), MockOpts { reconnect_ok: false, ..MockOpts::default() });
        assert_eq!(err_code(h.commander.reconnect(&json!({})).await), "RECONNECT_FAILED");
    }

    #[tokio::test]
    async fn device_unavailable_when_the_task_is_gone() {
        // Drop the receiver so the control channel is closed.
        let (tx, rx) = mpsc::channel::<DeviceControl>(1);
        drop(rx);
        let cfg = a_device();
        let health = Arc::new(Health::default());
        let dm = make_dm(&cfg, Arc::clone(&health));
        let handle = DeviceHandle { cfg, control: tx, health, dm, signals: sim_signals() };
        let commander = Commander::new(vec![handle]);
        assert_eq!(err_code(commander.reconnect(&json!({})).await), "DEVICE_UNAVAILABLE");
    }

    // --- panels ------------------------------------------------------------------------------------

    #[test]
    fn the_three_panels_are_registered_with_the_right_ids_orders_and_scope() {
        let ps = panels();
        let ids: Vec<&str> = ps.iter().map(|p| p["id"].as_str().unwrap()).collect();
        assert_eq!(ids, vec!["overview", "signals", "diagnostics"]);
        let orders: Vec<u64> = ps.iter().map(|p| p["order"].as_u64().unwrap()).collect();
        assert_eq!(orders, vec![10, 20, 30]);
        for p in &ps {
            assert_eq!(p["scope"], json!("instance"), "every panel is instance-scoped");
        }
        // The signals panel binds the signal verbs; diagnostics binds browse.
        assert_eq!(ps[1]["verbs"], json!(["sb/signals", "sb/read", "sb/write", "repoll"]));
        assert_eq!(ps[2]["verbs"], json!(["sb/browse", "sb/status"]));
    }
}
