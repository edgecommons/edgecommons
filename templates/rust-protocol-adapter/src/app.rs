//! # <<COMPONENTNAME>> — a southbound protocol adapter
//!
//! An **adapter** connects to devices, reads signals, and publishes them onto the UNS in the
//! shape the rest of the fleet expects — so that a consumer can chart a Modbus register and an
//! OPC UA node without knowing either protocol.
//!
//! ```text
//!   connect ──► poll ──► publish SouthboundSignalUpdate ──► report health
//!      ▲                                                         │
//!      └──────────── reconnect with backoff ◄────────────────────┘
//! ```
//!
//! One task per instance: an instance is one device, and its connection lifecycle is its own. That
//! task also owns a **control channel** ([`DeviceControl`]) — every command that must touch the
//! (non-`Sync`) session or serialize with the poll loop is *sent* to the task, and *confirmed*
//! through the reply that rides it. The command surface itself lives in [`crate::commands`].
//!
//! ## The contract you are implementing (docs/SOUTHBOUND.md)
//!
//! * Publish `SouthboundSignalUpdate` on the `data` class, **via the `data()` facade** — never
//!   hand-build the body and never hand-write the topic.
//! * **Quality on every sample**, normalized to `GOOD | BAD | UNCERTAIN`, with the native code in
//!   `qualityRaw`.
//! * Emit **`southbound_health`** (the exact §5 set — see [`crate::metrics`]), dimensioned by
//!   instance, so an operator can see a link go down without reading logs.
//! * Report **per-instance connectivity** ([`connectivity_of`]).
//! * Serve **read/write/browse/reconnect/pause commands** — and allow-list the writes.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use edgecommons::prelude::*;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::{mpsc, oneshot};

use crate::device::{
    BrowseError, BrowsePage, ConnectionConfig, DeviceBackend, Quality, Reading, SimBackend,
};
use crate::metrics::DeviceMetrics;

/// How often the periodic metrics emit runs, in the poll loop.
const METRICS_INTERVAL: Duration = Duration::from_secs(30);
/// The `component.global.healthThresholds.staleSignalSecs` default (SOUTHBOUND.md §4/§5).
const DEFAULT_STALE_SIGNAL_SECS: u64 = 30;

/// One device == one entry of `component.instances[]`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeviceConfig {
    /// The instance id. It is the `{instance}` token of this device's UNS topics, so it must be a
    /// valid UNS token (lower-kebab).
    pub id: String,
    /// Which backend to use. Matches [`crate::device::DeviceBackend::kind`].
    #[serde(default = "default_adapter")]
    pub adapter: String,
    pub connection: ConnectionConfig,
    /// How often to read, in milliseconds.
    #[serde(default = "default_poll_ms")]
    pub poll_interval_ms: u64,
    /// Writes are **allow-listed by stable `signal.id`**. An empty list means this adapter is
    /// read-only, which is the correct default for anything touching a control system.
    #[serde(default)]
    pub writes: Writes,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Writes {
    /// Signal ids this adapter is permitted to write. Nothing else is writable, whatever the
    /// command asks for.
    #[serde(default)]
    pub allow: Vec<String>,
}

impl Writes {
    #[must_use]
    pub fn permits(&self, signal_id: &str) -> bool {
        self.allow.iter().any(|s| s == signal_id)
    }
}

fn default_adapter() -> String {
    "sim".into()
}
fn default_poll_ms() -> u64 {
    5_000
}

/// Reconnect backoff. Exponential with full jitter and a cap — so a site whose PLC reboots does
/// not get every adapter in the plant reconnecting in lockstep on the same second.
#[derive(Debug, Clone, Copy)]
pub struct Backoff {
    pub base_ms: u64,
    pub max_ms: u64,
}

impl Default for Backoff {
    fn default() -> Self {
        Self { base_ms: 1_000, max_ms: 60_000 }
    }
}

impl Backoff {
    #[must_use]
    pub fn delay(&self, attempt: u32, rand01: f64) -> Duration {
        let exp = self.base_ms.saturating_mul(1_u64 << attempt.min(20));
        let cap = exp.min(self.max_ms);
        Duration::from_millis((rand01.clamp(0.0, 1.0) * cap as f64) as u64)
    }
}

/// This adapter's **own vocabulary** for a link's condition — what it reports as
/// `InstanceConnectivity::state`. A boolean cannot tell "still trying" from "backing off after a
/// failure"; an operator needs to, so the richer token exists alongside the normalized flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum LinkState {
    /// Connecting for the first time; nothing has failed yet.
    #[default]
    Connecting = 0,
    /// The session is up and being polled.
    Online = 1,
    /// The link failed; reconnecting with backoff.
    Backoff = 2,
}

impl LinkState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Connecting => "CONNECTING",
            Self::Online => "ONLINE",
            Self::Backoff => "BACKOFF",
        }
    }

    fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Online,
            2 => Self::Backoff,
            _ => Self::Connecting,
        }
    }
}

/// The shared per-device state the metrics emitter reads and the connectivity provider renders.
/// The gauges (`connection_state`, latencies) and the interval counters (`read_errors`, `reconnects`)
/// feed `southbound_health` ([`crate::metrics`]); `paused` and `link` feed the connectivity token and
/// `sb/status`. One source, several surfaces — so a health dot, a metric, and a status reply can
/// never disagree.
#[derive(Default)]
pub struct Health {
    /// 1 = connected, 0 = down.
    pub connection_state: AtomicU64,
    /// The [`LinkState`], as a `u8`. Read it through [`Health::link`].
    link: AtomicU8,
    /// 1 = telemetry production is paused (`sb/pause`). Read by the connectivity provider and
    /// `sb/status`; NOT a `southbound_health` measure (§5 has no `paused`).
    pub paused: AtomicBool,
    pub poll_latency_ms: AtomicU64,
    pub publish_latency_ms: AtomicU64,
    pub read_errors: AtomicU64,
    pub reconnects: AtomicU64,
}

impl Health {
    /// Record the link's condition. The metric's boolean and the reported state token move
    /// **together**, so the health dot and the label a console shows can never disagree.
    pub fn set_link(&self, state: LinkState) {
        self.link.store(state as u8, Ordering::Relaxed);
        self.connection_state
            .store(u64::from(state == LinkState::Online), Ordering::Relaxed);
    }

    #[must_use]
    pub fn link(&self) -> LinkState {
        LinkState::from_u8(self.link.load(Ordering::Relaxed))
    }

    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }
}

/// Flip the paused flag, returning whether the state actually changed (idempotent — pausing an
/// already-paused device is not an error). The event is emitted by the caller, which holds the
/// `events()` facade.
#[must_use]
pub fn set_paused(health: &Health, paused: bool) -> bool {
    health.paused.swap(paused, Ordering::Relaxed) != paused
}

/// One device's connectivity sample, for the instance-connectivity provider registered in
/// [`App::run`].
///
/// * `connected` is the **normalized** flag — always present.
/// * `state` is *this adapter's* vocabulary ([`LinkState`]) — `PAUSED` when paused and up, else the
///   raw link token (so a break while paused still reads `BACKOFF`, `connected` staying truthful).
/// * `attributes` is the **open** bag: domain data only this adapter understands.
#[must_use]
pub fn connectivity_of(cfg: &DeviceConfig, health: &Health) -> InstanceConnectivity {
    let link = health.link();
    let connected = link == LinkState::Online;
    let paused = health.is_paused();
    let state = if paused && connected { "PAUSED" } else { link.as_str() };

    let mut attributes = serde_json::Map::new();
    attributes.insert("adapter".to_string(), json!(cfg.adapter));
    attributes.insert("paused".to_string(), json!(paused));

    InstanceConnectivity::new(&cfg.id, connected, Some(cfg.connection.endpoint.clone()))
        .with_state(state)
        .with_attributes(attributes)
}

// =================================================================================================
// The device control channel
// =================================================================================================

/// A confirmed, allow-listed write of one signal, on its way from the command inbox to the device's
/// own task (`sb/write`).
pub struct WriteRequest {
    pub signal_id: String,
    pub value: serde_json::Value,
    /// The device's answer. A write is confirmed, not fire-and-forget.
    pub ack: oneshot::Sender<std::result::Result<(), String>>,
}

/// One message on a device's **control channel**. Every `sb/*` verb that must touch the session or
/// serialize with the poll loop is delivered as one of these, so the command inbox never touches the
/// (non-`Sync`) session directly — the device's own task services them one at a time. The reply
/// riding each variant is what makes reads/writes/reconnect *confirmed*.
pub enum DeviceControl {
    /// A confirmed, allow-listed write (`sb/write`). The allow-list is checked in the command layer
    /// BEFORE this is ever sent.
    Write(WriteRequest),
    /// Live-read these ids now (`sb/read`). Serializes with the loop and works while paused.
    ReadNow {
        ids: Vec<String>,
        reply: oneshot::Sender<std::result::Result<Vec<Reading>, String>>,
    },
    /// One page of address-space discovery (`sb/browse`).
    Browse {
        cursor: Option<String>,
        max: usize,
        reply: oneshot::Sender<std::result::Result<BrowsePage, BrowseError>>,
    },
    /// Pause telemetry production (`sb/pause`). Reply = whether the state changed.
    Pause { reply: oneshot::Sender<bool> },
    /// Resume telemetry production (`sb/resume`). Reply = whether the state changed.
    Resume { reply: oneshot::Sender<bool> },
    /// Drop + re-establish, one immediate attempt (`reconnect`). `Ok(())` ⇒ connected, `Err` ⇒
    /// failed (mapped to `RECONNECT_FAILED`).
    Reconnect { reply: oneshot::Sender<std::result::Result<(), String>> },
    /// Force an immediate poll now (`repoll`). Reply = signals read, or `Err` when refused (paused).
    Repoll { reply: oneshot::Sender<std::result::Result<u64, String>> },
}

// =================================================================================================
// App
// =================================================================================================

pub struct App {
    config: Arc<Config>,
    metrics: Arc<dyn MetricService>,
    devices: Vec<DeviceConfig>,
    /// `component.global.healthThresholds.staleSignalSecs`.
    stale_signal_secs: u64,
}

struct ConfigListener;

#[async_trait::async_trait]
impl ConfigurationChangeListener for ConfigListener {
    async fn on_configuration_change(&self, config: Arc<Config>) -> bool {
        tracing::info!(identity = %config.identity().path(), "configuration changed");
        true
    }
}

impl App {
    pub fn new(gg: &EdgeCommons) -> anyhow::Result<Self> {
        gg.add_config_change_listener(Arc::new(ConfigListener));

        let config = gg.config();
        let metrics = gg.metrics();

        let stale_signal_secs = config
            .global()
            .get("healthThresholds")
            .and_then(|h| h.get("staleSignalSecs"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(DEFAULT_STALE_SIGNAL_SECS);

        let mut devices = Vec::new();
        for id in config.instance_ids() {
            match config
                .instance(&id)
                .ok_or_else(|| anyhow::anyhow!("no config"))
                .and_then(|v| Ok(serde_json::from_value::<DeviceConfig>(v.clone())?))
            {
                Ok(d) => devices.push(d),
                Err(e) => tracing::warn!("skipping malformed device `{id}`: {e}"),
            }
        }
        anyhow::ensure!(!devices.is_empty(), "no valid devices in component.instances[]");

        Ok(Self { config, metrics, devices, stale_signal_secs })
    }

    pub async fn run(&self, gg: &EdgeCommons) -> anyhow::Result<()> {
        // Each device's health, shared with its task and read by the connectivity provider.
        let mut reported: Vec<(DeviceConfig, Arc<Health>)> = Vec::new();
        // The per-device handles the command surface routes on.
        let mut handles: Vec<crate::commands::DeviceHandle> = Vec::new();

        for device in &self.devices {
            // Per-instance facades: `data()` mints this device's topics and stamps its identity.
            let instance = gg.instance(&device.id)?;

            let health = Arc::new(Health::default());
            let dm = Arc::new(DeviceMetrics::new(
                Arc::clone(&self.metrics),
                Arc::clone(&self.config),
                device.id.clone(),
                Arc::clone(&health),
                self.stale_signal_secs,
            ));
            // Pre-define the metric set so it is fixed and discoverable at startup.
            dm.define_all();

            // The signal inventory `sb/signals` shows — a config/backend view, no device round-trip.
            let signals = make_backend(device)
                .map(|b| b.inventory(&device.connection))
                .unwrap_or_default();

            let (control_tx, control_rx) = mpsc::channel::<DeviceControl>(16);
            reported.push((device.clone(), Arc::clone(&health)));
            handles.push(crate::commands::DeviceHandle {
                cfg: device.clone(),
                control: control_tx,
                health: Arc::clone(&health),
                dm: Arc::clone(&dm),
                signals,
            });

            tokio::spawn(run_device(
                device.clone(),
                instance.data(),
                instance.events(),
                dm,
                health,
                control_rx,
            ));
        }

        // ONE provider, TWO surfaces: the library pushes this sample into the `state` keepalive's
        // `instances[]` every tick, and returns the very same sample from the built-in `status`
        // command verb. Whoever watches and whoever asks cannot get different answers.
        let provider: Arc<InstanceConnectivityProvider> = Arc::new(move || {
            reported.iter().map(|(cfg, health)| connectivity_of(cfg, health)).collect()
        });
        gg.set_instance_connectivity_provider(Some(provider));

        // The southbound command surface (`crate::commands`). `ping` / `reload-config` /
        // `get-configuration` are already live — the library registered them before we ran.
        if let Some(commands) = gg.commands() {
            crate::commands::register_all(&commands, handles)?;
        }

        gg.shutdown_signal().await;
        tracing::info!("shutdown signal received");
        self.metrics.flush_metrics().await.ok();
        Ok(())
    }
}

/// Instantiate the backend for a device's `adapter`. A real adapter matches its protocol(s) here.
fn make_backend(cfg: &DeviceConfig) -> Option<Box<dyn DeviceBackend>> {
    match cfg.adapter.as_str() {
        "sim" => Some(Box::new(SimBackend)),
        other => {
            tracing::error!(instance = %cfg.id, adapter = %other, "unknown adapter");
            None
        }
    }
}

// =================================================================================================
// The device task
// =================================================================================================

/// One device's lifecycle: connect, poll, publish, reconnect — and service its control channel.
///
/// The connect loop and the poll loop are nested on purpose. A read failure that breaks the link
/// drops out of the poll loop and back into connect — the only place that knows how to back off.
async fn run_device(
    cfg: DeviceConfig,
    data: DataFacade,
    events: EventsFacade,
    dm: Arc<DeviceMetrics>,
    health: Arc<Health>,
    mut control: mpsc::Receiver<DeviceControl>,
) {
    let Some(backend) = make_backend(&cfg) else {
        return;
    };
    let backoff = Backoff::default();
    let mut attempt: u32 = 0;
    // A `reconnect` command's reply, held until the next connect settles it.
    let mut pending_reconnect: Option<oneshot::Sender<std::result::Result<(), String>>> = None;

    loop {
        // --- CONNECT (servicing control while down, so pause/reconnect don't block on backoff) ---
        let session = loop {
            dm.on_connect_attempt();
            health.set_link(if attempt == 0 { LinkState::Connecting } else { LinkState::Backoff });
            let now = Instant::now();
            match backend.connect(&cfg.connection).await {
                Ok(session) => {
                    attempt = 0;
                    dm.on_connected(now);
                    health.set_link(LinkState::Online);
                    dm.emit_now().await;
                    let _ = events
                        .emit(
                            Severity::Info,
                            "device-connected",
                            Some(format!("connected to {}", cfg.connection.endpoint)),
                            Some(json!({ "instance": cfg.id, "adapter": backend.kind() })),
                        )
                        .await;
                    let _ = events.clear_alarm(Severity::Critical, "device-unreachable", None).await;
                    if let Some(reply) = pending_reconnect.take() {
                        let _ = reply.send(Ok(()));
                    }
                    break session;
                }
                Err(e) => {
                    dm.on_connect_failure();
                    if let Some(reply) = pending_reconnect.take() {
                        let _ = reply.send(Err(e.to_string()));
                    }
                    // A permanent failure fails identically forever — back off to the ceiling.
                    let permanent = !e.is_transient();
                    let wait = if permanent {
                        Duration::from_millis(backoff.max_ms)
                    } else {
                        backoff.delay(attempt, rand01())
                    };
                    attempt = attempt.saturating_add(1);
                    tracing::warn!(
                        instance = %cfg.id, error = %e, permanent,
                        wait_ms = wait.as_millis() as u64, "connect failed"
                    );
                    match serve_while_down(&mut control, &events, &health, wait).await {
                        DownOutcome::Reconnect(reply) => {
                            pending_reconnect = Some(reply);
                            attempt = 0;
                        }
                        DownOutcome::Elapsed => {}
                        DownOutcome::Closed => return,
                    }
                }
            }
        };

        // --- POLL (until the link breaks or a reconnect is requested) ---
        let exit = run_polling(&cfg, session, &data, &events, &dm, &health, &mut control).await;

        // The link is down (or a reconnect asked us to drop it).
        health.set_link(LinkState::Backoff);
        health.reconnects.fetch_add(1, Ordering::Relaxed);
        dm.on_connection_dropped(Instant::now());
        dm.emit_now().await;
        let _ = events
            .raise_alarm(
                Severity::Critical,
                "device-unreachable",
                Some(format!("lost the link to {}", cfg.connection.endpoint)),
                Some(json!({ "instance": cfg.id })),
            )
            .await;

        match exit {
            PollExit::LinkLost => {}
            PollExit::Reconnect(reply) => {
                pending_reconnect = Some(reply);
            }
            PollExit::Closed => return,
        }
    }
}

/// What ended the poll loop.
enum PollExit {
    /// A read broke the connection; reconnect via the connect loop.
    LinkLost,
    /// A `reconnect` command asked us to drop + re-establish; settle its reply on the next connect.
    Reconnect(oneshot::Sender<std::result::Result<(), String>>),
    /// The control channel closed (component shutdown).
    Closed,
}

/// Read on the poll interval and publish, servicing the control channel, until the link breaks or a
/// reconnect is requested.
async fn run_polling(
    cfg: &DeviceConfig,
    mut session: Box<dyn crate::device::DeviceSession>,
    data: &DataFacade,
    events: &EventsFacade,
    dm: &Arc<DeviceMetrics>,
    health: &Arc<Health>,
    control: &mut mpsc::Receiver<DeviceControl>,
) -> PollExit {
    let mut ticker = tokio::time::interval(Duration::from_millis(cfg.poll_interval_ms));
    let mut since_metrics = Instant::now();

    loop {
        tokio::select! {
            // Poll and control share this one task, so a write can never race a read on the same
            // connection — most device protocols are a single request/response channel.
            ctrl = control.recv() => {
                let Some(ctrl) = ctrl else { return PollExit::Closed; };
                match ctrl {
                    DeviceControl::Write(req) => {
                        let result = session
                            .write_signal(&req.signal_id, &req.value)
                            .await
                            .map_err(|e| e.to_string());
                        if let Err(e) = &result {
                            tracing::warn!(instance = %cfg.id, signal = %req.signal_id, error = %e, "write failed");
                        }
                        let _ = req.ack.send(result);
                    }
                    DeviceControl::ReadNow { ids, reply } => {
                        let result = session.read_named(&ids).await.map_err(|e| e.to_string());
                        let _ = reply.send(result);
                    }
                    DeviceControl::Browse { cursor, max, reply } => {
                        let _ = reply.send(session.browse(cursor, max).await);
                    }
                    DeviceControl::Pause { reply } => {
                        let changed = set_paused(health, true);
                        if changed {
                            let _ = events
                                .emit(
                                    Severity::Warning,
                                    "adapter-paused",
                                    Some("telemetry production paused".to_string()),
                                    Some(json!({ "instance": cfg.id })),
                                )
                                .await;
                        }
                        let _ = reply.send(changed);
                    }
                    DeviceControl::Resume { reply } => {
                        let changed = set_paused(health, false);
                        if changed {
                            let _ = events
                                .emit(
                                    Severity::Info,
                                    "adapter-resumed",
                                    Some("telemetry production resumed".to_string()),
                                    Some(json!({ "instance": cfg.id })),
                                )
                                .await;
                        }
                        let _ = reply.send(changed);
                    }
                    DeviceControl::Reconnect { reply } => {
                        session.close().await;
                        return PollExit::Reconnect(reply);
                    }
                    DeviceControl::Repoll { reply } => {
                        if health.is_paused() {
                            let _ = reply.send(Err("instance is paused - resume first".to_string()));
                        } else {
                            match poll_once(cfg, &mut session, data, dm, health).await {
                                Ok(n) => {
                                    let _ = reply.send(Ok(n));
                                }
                                Err(()) => {
                                    let _ = reply.send(Err("link error".to_string()));
                                    session.close().await;
                                    return PollExit::LinkLost;
                                }
                            }
                        }
                    }
                }
            }

            _ = ticker.tick(), if !health.is_paused() => {
                if poll_once(cfg, &mut session, data, dm, health).await.is_err() {
                    session.close().await;
                    return PollExit::LinkLost;
                }
            }
        }

        if since_metrics.elapsed() >= METRICS_INTERVAL {
            dm.emit_periodic().await;
            since_metrics = Instant::now();
        }
    }
}

/// One poll: read, publish each reading, record latencies + staleness. `Ok(n)` = signals published;
/// `Err(())` = the *connection* broke (caller reconnects).
async fn poll_once(
    cfg: &DeviceConfig,
    session: &mut Box<dyn crate::device::DeviceSession>,
    data: &DataFacade,
    dm: &Arc<DeviceMetrics>,
    health: &Arc<Health>,
) -> std::result::Result<u64, ()> {
    let backend_adapter = cfg.adapter.clone();
    let started = Instant::now();
    let readings = match session.read_signals().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(instance = %cfg.id, error = %e, "read failed; reconnecting");
            health.read_errors.fetch_add(1, Ordering::Relaxed);
            return Err(());
        }
    };
    let latency = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    health.poll_latency_ms.store(latency, Ordering::Relaxed);

    let publish_started = Instant::now();
    let mut published = 0u64;
    for r in readings {
        // The data() facade builds the SouthboundSignalUpdate body, mints the topic, and stamps
        // identity. Do not hand-build any of the three.
        let quality = match r.quality {
            Quality::Good => edgecommons::facades::Quality::Good,
            Quality::Bad => edgecommons::facades::Quality::Bad,
            Quality::Uncertain => edgecommons::facades::Quality::Uncertain,
        };
        let mut sample = Sample::with_quality(r.value.clone(), quality);
        if let Some(raw) = &r.quality_raw {
            sample = sample.quality_raw(raw);
        }

        let mut signal = data.signal(&r.signal_id);
        if let Some(name) = &r.name {
            signal = signal.name(name);
        }
        let update = signal
            .device_parts(&backend_adapter, &cfg.id, &cfg.connection.endpoint)
            .sample(sample)
            .build();

        if let Err(e) = data.publish(update).await {
            tracing::warn!(instance = %cfg.id, signal = %r.signal_id, error = %e, "publish failed");
        } else {
            published += 1;
            // Feed the staleness tracker — a signal that keeps updating is not stale.
            dm.on_signal_update(&r.signal_id, Instant::now());
        }
    }
    let publish_latency = u64::try_from(publish_started.elapsed().as_millis()).unwrap_or(u64::MAX);
    health.publish_latency_ms.store(publish_latency, Ordering::Relaxed);
    Ok(published)
}

/// What servicing the control channel while the session is down concluded.
enum DownOutcome {
    /// A `reconnect` command wants us to connect *now* (cut the backoff short); settle its reply on
    /// the next connect.
    Reconnect(oneshot::Sender<std::result::Result<(), String>>),
    /// The backoff window elapsed — retry the connect.
    Elapsed,
    /// The control channel closed (component shutdown).
    Closed,
}

/// Service the control channel while the session is **down**, for up to `wait`. Pause/resume take
/// effect (they only need the shared flag + event); the I/O verbs answer "disconnected" (the command
/// layer maps that to `DEVICE_UNAVAILABLE` / `BROWSE_FAILED`); a `reconnect` returns its reply so the
/// caller connects now.
async fn serve_while_down(
    control: &mut mpsc::Receiver<DeviceControl>,
    events: &EventsFacade,
    health: &Arc<Health>,
    wait: Duration,
) -> DownOutcome {
    let deadline = Instant::now() + wait;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return DownOutcome::Elapsed;
        }
        tokio::select! {
            biased;
            ctrl = control.recv() => {
                match ctrl {
                    None => return DownOutcome::Closed,
                    Some(DeviceControl::Reconnect { reply }) => return DownOutcome::Reconnect(reply),
                    Some(DeviceControl::Pause { reply }) => {
                        let changed = set_paused(health, true);
                        if changed {
                            let _ = events.emit(Severity::Warning, "adapter-paused", None, None).await;
                        }
                        let _ = reply.send(changed);
                    }
                    Some(DeviceControl::Resume { reply }) => {
                        let changed = set_paused(health, false);
                        if changed {
                            let _ = events.emit(Severity::Info, "adapter-resumed", None, None).await;
                        }
                        let _ = reply.send(changed);
                    }
                    Some(DeviceControl::Write(req)) => {
                        let _ = req.ack.send(Err("device is disconnected".to_string()));
                    }
                    Some(DeviceControl::ReadNow { reply, .. }) => {
                        let _ = reply.send(Err("device is disconnected".to_string()));
                    }
                    Some(DeviceControl::Repoll { reply }) => {
                        let _ = reply.send(Err("device is disconnected".to_string()));
                    }
                    Some(DeviceControl::Browse { reply, .. }) => {
                        let _ = reply.send(Err(BrowseError::Failed("device is disconnected".to_string())));
                    }
                }
            }
            _ = tokio::time::sleep(remaining) => return DownOutcome::Elapsed,
        }
    }
}

fn rand01() -> f64 {
    use std::hash::{BuildHasher, Hasher};
    let n = std::collections::hash_map::RandomState::new().build_hasher().finish();
    (n % 1_000_000) as f64 / 1_000_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_device_parses_from_its_instance_config() {
        let d: DeviceConfig = serde_json::from_value(json!({
            "id": "plc-1",
            "adapter": "sim",
            "connection": { "endpoint": "sim://plc-1", "unitId": 3 },
            "pollIntervalMs": 1000,
            "writes": { "allow": ["setpoint-1"] }
        }))
        .unwrap();

        assert_eq!(d.id, "plc-1");
        assert_eq!(d.poll_interval_ms, 1_000);
        // `connection` is deliberately open: every protocol needs different keys.
        assert_eq!(d.connection.extra["unitId"], 3);
    }

    #[test]
    fn an_adapter_is_read_only_until_a_write_is_allow_listed() {
        // The default must be read-only. An adapter that writes any address it is asked to is a
        // control-system vulnerability, not a convenience.
        let d: DeviceConfig = serde_json::from_value(json!({
            "id": "plc-1",
            "connection": { "endpoint": "sim://plc-1" }
        }))
        .unwrap();
        assert!(!d.writes.permits("setpoint-1"), "nothing is writable by default");

        let w = Writes { allow: vec!["setpoint-1".into()] };
        assert!(w.permits("setpoint-1"));
        assert!(!w.permits("setpoint-2"), "only the listed signal, not its neighbours");
    }

    #[test]
    fn reconnect_backoff_is_exponential_capped_and_jittered() {
        let b = Backoff { base_ms: 1_000, max_ms: 10_000 };
        assert_eq!(b.delay(0, 1.0).as_millis(), 1_000);
        assert_eq!(b.delay(2, 1.0).as_millis(), 4_000);
        assert_eq!(b.delay(20, 1.0).as_millis(), 10_000, "capped");
        // Jitter: the delay is a point in the window, not its edge.
        assert_eq!(b.delay(2, 0.5).as_millis(), 2_000);
        assert_eq!(b.delay(2, 0.0).as_millis(), 0);
    }

    #[test]
    fn an_unknown_config_key_is_rejected_rather_than_ignored() {
        let bad = serde_json::from_value::<DeviceConfig>(json!({
            "id": "plc-1",
            "connection": { "endpoint": "x" },
            "pollIntervalMS": 1000
        }));
        assert!(bad.is_err(), "a typo'd key is a mistake, not a no-op");
    }

    #[test]
    fn every_device_reports_its_own_connectivity() {
        let cfg: DeviceConfig = serde_json::from_value(json!({
            "id": "plc-1",
            "adapter": "sim",
            "connection": { "endpoint": "sim://plc-1" }
        }))
        .unwrap();
        let health = Health::default();

        // Before the first connect: not reachable, and the token says why — CONNECTING, not BACKOFF.
        let c = connectivity_of(&cfg, &health);
        assert_eq!(c.instance, "plc-1");
        assert!(!c.connected);
        assert_eq!(c.state.as_deref(), Some("CONNECTING"));
        assert_eq!(c.detail.as_deref(), Some("sim://plc-1"), "the endpoint, for a human");
        assert_eq!(c.attributes["adapter"], json!("sim"), "the open bag carries domain data");
        assert_eq!(c.attributes["paused"], json!(false));

        health.set_link(LinkState::Online);
        let c = connectivity_of(&cfg, &health);
        assert!(c.connected, "the normalized flag every console reads");
        assert_eq!(c.state.as_deref(), Some("ONLINE"));

        health.set_link(LinkState::Backoff);
        assert!(!connectivity_of(&cfg, &health).connected);
    }

    #[test]
    fn a_paused_online_device_reports_paused_but_stays_connected() {
        let cfg: DeviceConfig = serde_json::from_value(json!({
            "id": "plc-1", "connection": { "endpoint": "sim://plc-1" }
        }))
        .unwrap();
        let health = Health::default();
        health.set_link(LinkState::Online);

        assert!(set_paused(&health, true), "pausing changed the state");
        assert!(!set_paused(&health, true), "pausing again is idempotent");
        let c = connectivity_of(&cfg, &health);
        assert_eq!(c.state.as_deref(), Some("PAUSED"), "paused + online = PAUSED");
        assert!(c.connected, "connected stays truthful while paused");
        assert_eq!(c.attributes["paused"], json!(true));

        // A break while paused reports BACKOFF (not PAUSED), `connected` false.
        health.set_link(LinkState::Backoff);
        let c = connectivity_of(&cfg, &health);
        assert_eq!(c.state.as_deref(), Some("BACKOFF"));
        assert!(!c.connected);
    }

    #[test]
    fn the_normalized_flag_and_the_health_metric_cannot_disagree() {
        let health = Health::default();
        health.set_link(LinkState::Online);
        assert_eq!(health.connection_state.load(Ordering::Relaxed), 1);
        health.set_link(LinkState::Backoff);
        assert_eq!(health.connection_state.load(Ordering::Relaxed), 0);
    }
}
