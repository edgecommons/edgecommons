//! # <<COMPONENTNAME>> — a sink component
//!
//! A **sink** is the last thing standing between data and its destination. It consumes work,
//! delivers it outward, and only then lets go of the source.
//!
//! ```text
//!   consume ──► deliver (idempotent, stable key) ──► verify ──► confirm ──► report
//!                        ▲                                                    │
//!                        └────────── retry with full jitter ◄─────────────────┘
//! ```
//!
//! The ordering is the archetype, and every step earns its place:
//!
//! * **Deliver idempotently, to a stable key.** A redelivery overwrites; it does not duplicate.
//!   A sink that cannot retry without duplicating cannot retry at all.
//! * **Verify before you confirm.** Trusting `deliver`'s `Ok` and releasing the source without
//!   checking what actually landed is how you end up having deleted the only copy.
//! * **Classify the failure.** Retrying a permanent error burns the budget; giving up on a
//!   transient one loses data a second attempt would have delivered. See
//!   [`crate::dest::DeliverError`].
//! * **Report every transition.** A sink that fails quietly is indistinguishable from one that is
//!   idle. Started / completed / failed / exhausted all go out on the UNS event surface — and the
//!   same transitions move each destination's reported connectivity ([`connectivity_of`]), because
//!   a sink's destinations **are** its instances.
//!
//! ## Where the work comes from
//!
//! This scaffold's source is a **subscription**: it consumes messages off the bus and delivers
//! each one. That is the common case. If your source is a watched directory or a polled API,
//! replace [`App::run`]'s subscribe call — everything downstream of `deliver_with_retry` is
//! unchanged, which is the point of the seam.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::time::Duration;

use edgecommons::messaging::Message;
use edgecommons::prelude::*;
use serde::Deserialize;
use serde_json::json;

use crate::dest::{DestinationConfig, Item, SharedDestination};

const METRIC_NAME: &str = "sinkDeliveries";

/// One sink instance == one entry of `component.instances[]`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SinkConfig {
    pub id: String,
    /// The topic filter whose messages this sink delivers.
    pub subscribe: String,
    /// Where they go.
    pub destination: DestinationConfig,
    #[serde(default)]
    pub retry: RetryConfig,
    /// Bounded, like every queue that faces a network.
    #[serde(default = "default_max_queue")]
    pub max_queue: usize,
}

fn default_max_queue() -> usize {
    256
}

/// How hard, and for how long, to keep trying.
///
/// Note the give-up is a **time budget**, not an attempt count. "Twenty attempts" means something
/// different at 1 s and at 15 min of backoff; "keep trying for an hour" means the same thing at
/// every cadence, and it is what an operator can actually reason about.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RetryConfig {
    #[serde(default = "default_base_ms")]
    pub base_delay_ms: u64,
    #[serde(default = "default_max_ms")]
    pub max_delay_ms: u64,
    #[serde(default = "default_give_up_ms")]
    pub give_up_after_ms: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            base_delay_ms: default_base_ms(),
            max_delay_ms: default_max_ms(),
            give_up_after_ms: default_give_up_ms(),
        }
    }
}

fn default_base_ms() -> u64 {
    1_000
}
fn default_max_ms() -> u64 {
    900_000 // 15 min
}
fn default_give_up_ms() -> u64 {
    3_600_000 // 1 hour
}

impl RetryConfig {
    /// Full-jitter exponential backoff: a random delay in `[0, min(cap, base * 2^attempt))`.
    ///
    /// The jitter is not decoration. Without it, every component that lost the same endpoint
    /// retries at the same instant, and the endpoint — which is probably struggling already —
    /// is hit by a synchronized thundering herd on every backoff boundary.
    #[must_use]
    pub fn delay(&self, attempt: u32, rand01: f64) -> Duration {
        let exp = self.base_delay_ms.saturating_mul(1_u64 << attempt.min(20));
        let cap = exp.min(self.max_delay_ms);
        Duration::from_millis((rand01.clamp(0.0, 1.0) * cap as f64) as u64)
    }

    #[must_use]
    pub fn budget_spent(&self, elapsed: Duration) -> bool {
        elapsed >= Duration::from_millis(self.give_up_after_ms)
    }
}

#[derive(Default)]
pub struct Stats {
    pub received: AtomicU64,
    pub delivered: AtomicU64,
    pub retried: AtomicU64,
    /// Gave up. This is the number that matters: it is data that did not arrive.
    pub exhausted: AtomicU64,
    pub dropped: AtomicU64,
}

/// This sink's **own vocabulary** for a destination's condition — what it reports as
/// `InstanceConnectivity::state`. The delivery ladder in [`deliver_with_retry`] moves it, so what
/// the events say and what the connectivity says are the same story.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum DestState {
    /// Nothing has been delivered yet, so nothing has failed yet. Reported reachable until proven
    /// otherwise — an untried destination is not a broken one.
    #[default]
    Idle = 0,
    /// The last delivery was verified.
    Online = 1,
    /// A transient failure; retrying inside the time budget.
    Backoff = 2,
    /// Permanent, or the budget is spent. Not reachable, and no longer trying — this is the state
    /// an operator must be paged about, and the boolean alone cannot distinguish it from a retry.
    Failed = 3,
}

impl DestState {
    /// The normalized flag: is the destination taking data right now?
    #[must_use]
    pub fn connected(self) -> bool {
        matches!(self, Self::Idle | Self::Online)
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "IDLE",
            Self::Online => "ONLINE",
            Self::Backoff => "BACKOFF",
            Self::Failed => "FAILED",
        }
    }

    fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Online,
            2 => Self::Backoff,
            3 => Self::Failed,
            _ => Self::Idle,
        }
    }
}

/// One destination's condition: written by that sink's delivery task, read by the connectivity
/// provider registered in [`App::run`].
#[derive(Default)]
pub struct DestHealth(AtomicU8);

impl DestHealth {
    pub fn set(&self, state: DestState) {
        self.0.store(state as u8, Ordering::Relaxed);
    }

    #[must_use]
    pub fn get(&self) -> DestState {
        DestState::from_u8(self.0.load(Ordering::Relaxed))
    }
}

/// One destination's connectivity sample.
///
/// * `connected` is the **normalized** flag — always present, so a console can render a health dot
///   for this sink without knowing what an object store is.
/// * `state` is *this sink's* vocabulary ([`DestState`]): `BACKOFF` (still trying) and `FAILED`
///   (gave up) are the same boolean and very different pages at 3 a.m.
/// * `attributes` is the **open** bag: domain data only this sink understands (here, the kind of
///   backend), carried without destabilizing the two fields above that every consumer relies on.
#[must_use]
pub fn connectivity_of(sink: &SinkConfig, kind: &str, health: &DestHealth) -> InstanceConnectivity {
    let state = health.get();
    let mut attributes = serde_json::Map::new();
    attributes.insert("destination".to_string(), json!(kind));

    InstanceConnectivity::new(&sink.id, state.connected(), Some(sink.destination.endpoint()))
        .with_state(state.as_str())
        .with_attributes(attributes)
}

pub struct App {
    metrics: Arc<dyn MetricService>,
    sinks: Vec<SinkConfig>,
    stats: Arc<Stats>,
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

        // `component.global.defaults` applies to every instance that does not override it.
        // A knob the schema promises and the code ignores is worse than no knob at all.
        let defaults = config.global().get("defaults").cloned().unwrap_or_default();

        metrics.define_metric(
            MetricBuilder::create(METRIC_NAME)
                .with_config(&config)
                .add_measure("received", "Count", 60)
                .add_measure("delivered", "Count", 60)
                .add_measure("retried", "Count", 60)
                .add_measure("exhausted", "Count", 60)
                .add_measure("dropped", "Count", 60)
                .build(),
        );

        let mut sinks = Vec::new();
        for id in config.instance_ids() {
            match config
                .instance(&id)
                .ok_or_else(|| anyhow::anyhow!("no config"))
                .and_then(|v| Ok(serde_json::from_value::<SinkConfig>(v.clone())?))
            {
                Ok(mut sink) => {
                    apply_defaults(&mut sink, &defaults);
                    sinks.push(sink);
                }
                Err(e) => tracing::warn!("skipping malformed sink `{id}`: {e}"),
            }
        }
        anyhow::ensure!(!sinks.is_empty(), "no valid sinks in component.instances[]");

        Ok(Self { metrics, sinks, stats: Arc::new(Stats::default()) })
    }

    pub async fn run(&self, gg: &EdgeCommons) -> anyhow::Result<()> {
        let Ok(messaging) = gg.messaging() else {
            anyhow::bail!("a sink needs a messaging transport, and none was wired");
        };

        // Each destination's condition, shared with its delivery task: the task writes it, the
        // connectivity provider below reads it.
        let mut reported: Vec<(SinkConfig, &'static str, Arc<DestHealth>)> = Vec::new();

        for sink in &self.sinks {
            let destination = crate::dest::build(&sink.destination)?;
            let health = Arc::new(DestHealth::default());
            reported.push((sink.clone(), destination.kind(), Arc::clone(&health)));
            let (tx, rx) = tokio::sync::mpsc::channel::<Item>(sink.max_queue);

            let stats = Arc::clone(&self.stats);
            let sink_id = sink.id.clone();
            messaging
                .subscribe(
                    &sink.subscribe,
                    message_handler(move |topic: String, msg: Message| {
                        let tx = tx.clone();
                        let stats = Arc::clone(&stats);
                        let sink_id = sink_id.clone();
                        async move {
                            stats.received.fetch_add(1, Ordering::Relaxed);
                            let item = Item {
                                // A stable, deterministic key: the same message always lands in
                                // the same place, so a redelivery overwrites.
                                key: key_for(&sink_id, &topic, &msg),
                                bytes: serde_json::to_vec(&msg.body).unwrap_or_default(),
                            };
                            if tx.try_send(item).is_err() {
                                stats.dropped.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }),
                    sink.max_queue,
                    1,
                )
                .await?;
            tracing::info!(sink = %sink.id, filter = %sink.subscribe, "subscribed");

            tokio::spawn(run_sink(
                sink.clone(),
                rx,
                destination,
                Arc::clone(&self.stats),
                health,
                gg.events(),
            ));
        }

        // ONE provider, TWO surfaces: the library pushes this sample into the `state` keepalive's
        // `instances[]` every tick, and returns the very same sample from the built-in `status`
        // command verb when a console asks. Whoever watches and whoever asks cannot get different
        // answers. Keep it cheap — it is sampled on the keepalive interval.
        //
        // A sink's destinations ARE its instances: one entry each, so the fleet sees a bucket stop
        // accepting data without reading a single log line.
        let provider: Arc<InstanceConnectivityProvider> = Arc::new(move || {
            reported
                .iter()
                .map(|(sink, kind, health)| connectivity_of(sink, kind, health))
                .collect()
        });
        gg.set_instance_connectivity_provider(Some(provider));

        let mut ticker = tokio::time::interval(Duration::from_secs(60));
        loop {
            tokio::select! {
                _ = ticker.tick() => self.emit_metrics().await,
                _ = gg.shutdown_signal() => {
                    tracing::info!("shutdown signal received");
                    break;
                }
            }
        }
        self.metrics.flush_metrics().await.ok();
        Ok(())
    }

    async fn emit_metrics(&self) {
        let mut v = HashMap::new();
        v.insert("received".to_string(), self.stats.received.swap(0, Ordering::Relaxed) as f64);
        v.insert("delivered".to_string(), self.stats.delivered.swap(0, Ordering::Relaxed) as f64);
        v.insert("retried".to_string(), self.stats.retried.swap(0, Ordering::Relaxed) as f64);
        v.insert("exhausted".to_string(), self.stats.exhausted.swap(0, Ordering::Relaxed) as f64);
        v.insert("dropped".to_string(), self.stats.dropped.swap(0, Ordering::Relaxed) as f64);
        if let Err(e) = self.metrics.emit_metric(METRIC_NAME, v).await {
            tracing::warn!(error = %e, "metric emit failed");
        }
    }
}

async fn run_sink(
    sink: SinkConfig,
    mut rx: tokio::sync::mpsc::Receiver<Item>,
    destination: SharedDestination,
    stats: Arc<Stats>,
    health: Arc<DestHealth>,
    events: EventsFacade,
) {
    while let Some(item) = rx.recv().await {
        deliver_with_retry(&sink, &item, &destination, &stats, &health, &events).await;
    }
    tracing::info!(sink = %sink.id, "sink stopped");
}

/// Deliver one item, retrying transient failures until the time budget is spent.
///
/// The event ladder is the sink's contract with whoever is watching: **started**, then either
/// **completed**, or **failed** (with `willRetry`), and finally **exhausted** if the budget runs
/// out. An operator must be able to tell "still trying" from "gave up", and gave-up must be loud.
///
/// Every rung of that ladder also moves `health` — the same distinction, reported as this
/// destination's [`DestState`] on the `state` keepalive and to the `status` verb.
async fn deliver_with_retry(
    sink: &SinkConfig,
    item: &Item,
    destination: &SharedDestination,
    stats: &Arc<Stats>,
    health: &Arc<DestHealth>,
    events: &EventsFacade,
) {
    let started = std::time::Instant::now();
    let mut attempt: u32 = 0;

    let _ = events
        .emit(
            Severity::Info,
            "delivery-started",
            None,
            Some(json!({ "sink": sink.id, "key": item.key, "kind": destination.kind() })),
        )
        .await;

    loop {
        // deliver, then VERIFY. Only a verified delivery is a delivery.
        let outcome = match destination.deliver(item).await {
            Ok(d) => destination.verify(item, &d).await,
            Err(e) => Err(e),
        };

        match outcome {
            Ok(()) => {
                stats.delivered.fetch_add(1, Ordering::Relaxed);
                health.set(DestState::Online);
                let _ = events
                    .emit(
                        Severity::Info,
                        "delivery-completed",
                        None,
                        Some(json!({
                            "sink": sink.id,
                            "key": item.key,
                            "attempts": attempt + 1,
                            "elapsedMs": started.elapsed().as_millis(),
                        })),
                    )
                    .await;
                // The source is released HERE — after verification, never before.
                return;
            }

            // Permanent: it will fail identically forever. Retrying is a waste of the budget and
            // of the log; give up now and say so.
            Err(e) if !e.is_transient() => {
                stats.exhausted.fetch_add(1, Ordering::Relaxed);
                health.set(DestState::Failed);
                tracing::error!(sink = %sink.id, key = %item.key, error = %e, "permanent failure");
                let _ = events
                    .emit(
                        Severity::Critical,
                        "delivery-exhausted",
                        Some(format!("{} will never deliver {}", sink.id, item.key)),
                        Some(json!({ "sink": sink.id, "key": item.key, "reason": e.to_string() })),
                    )
                    .await;
                return;
            }

            Err(e) => {
                if sink.retry.budget_spent(started.elapsed()) {
                    stats.exhausted.fetch_add(1, Ordering::Relaxed);
                    health.set(DestState::Failed);
                    tracing::error!(sink = %sink.id, key = %item.key, attempt, "retry budget spent");
                    let _ = events
                        .emit(
                            Severity::Critical,
                            "delivery-exhausted",
                            Some(format!("{} gave up on {}", sink.id, item.key)),
                            Some(json!({
                                "sink": sink.id, "key": item.key,
                                "attempts": attempt + 1, "reason": e.to_string(),
                            })),
                        )
                        .await;
                    return;
                }

                let backoff = sink.retry.delay(attempt, rand01());
                stats.retried.fetch_add(1, Ordering::Relaxed);
                health.set(DestState::Backoff);
                tracing::warn!(
                    sink = %sink.id, key = %item.key, attempt,
                    backoff_ms = backoff.as_millis() as u64, error = %e,
                    "transient failure; retrying"
                );
                let _ = events
                    .emit(
                        Severity::Warning,
                        "delivery-failed",
                        None,
                        Some(json!({
                            "sink": sink.id, "key": item.key, "attempt": attempt + 1,
                            "willRetry": true, "nextAttemptInMs": backoff.as_millis(),
                        })),
                    )
                    .await;

                tokio::time::sleep(backoff).await;
                attempt = attempt.saturating_add(1);
            }
        }
    }
}

/// A stable, deterministic key for a message.
///
/// Deterministic is the whole point: the same message must always resolve to the same key, or a
/// retry duplicates instead of overwriting.
fn key_for(sink_id: &str, topic: &str, msg: &Message) -> String {
    let leaf = topic.rsplit('/').next().unwrap_or("message");
    format!("{sink_id}/{leaf}/{}.json", msg.header.uuid)
}

/// A cheap uniform `[0, 1)`, so the template needs no `rand` dependency.
fn rand01() -> f64 {
    use std::hash::{BuildHasher, Hasher};
    let n = std::collections::hash_map::RandomState::new().build_hasher().finish();
    (n % 1_000_000) as f64 / 1_000_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sink_parses_from_its_instance_config() {
        let sink: SinkConfig = serde_json::from_value(json!({
            "id": "archive",
            "subscribe": "ecv1/+/+/+/data/#",
            "destination": { "type": "local", "path": "/var/lib/out" },
            "retry": { "baseDelayMs": 500, "giveUpAfterMs": 60000 }
        }))
        .unwrap();

        assert_eq!(sink.id, "archive");
        assert_eq!(sink.retry.base_delay_ms, 500);
        assert_eq!(sink.retry.max_delay_ms, 900_000, "the unspecified field takes its default");
    }

    #[test]
    fn backoff_grows_exponentially_and_is_capped() {
        let r = RetryConfig { base_delay_ms: 1_000, max_delay_ms: 10_000, give_up_after_ms: 0 };
        // With full jitter, `rand01 = 1.0` yields the ceiling of the window.
        assert_eq!(r.delay(0, 1.0).as_millis(), 1_000);
        assert_eq!(r.delay(1, 1.0).as_millis(), 2_000);
        assert_eq!(r.delay(2, 1.0).as_millis(), 4_000);
        // ...and it is capped, so a long outage does not back off to next week.
        assert_eq!(r.delay(20, 1.0).as_millis(), 10_000);
    }

    #[test]
    fn jitter_spreads_the_retries() {
        // The point of full jitter: two components that lost the same endpoint do NOT retry in
        // lockstep. The delay is a random point in the window, not the window's edge.
        let r = RetryConfig { base_delay_ms: 1_000, max_delay_ms: 60_000, give_up_after_ms: 0 };
        assert_eq!(r.delay(3, 0.0).as_millis(), 0, "the window's floor is immediate");
        assert_eq!(r.delay(3, 0.5).as_millis(), 4_000, "half way into an 8s window");
        assert_eq!(r.delay(3, 1.0).as_millis(), 8_000);
    }

    #[test]
    fn the_give_up_is_a_time_budget_not_an_attempt_count() {
        let r = RetryConfig { base_delay_ms: 1, max_delay_ms: 1, give_up_after_ms: 5_000 };
        assert!(!r.budget_spent(Duration::from_secs(4)));
        assert!(r.budget_spent(Duration::from_secs(5)));
    }

    fn sink_config(id: &str) -> SinkConfig {
        serde_json::from_value(json!({
            "id": id,
            "subscribe": "ecv1/+/+/+/data/#",
            "destination": { "type": "local", "path": "/var/lib/out" }
        }))
        .unwrap()
    }

    #[test]
    fn every_destination_reports_its_own_connectivity() {
        let sink = sink_config("archive");
        let health = DestHealth::default();

        // Nothing delivered yet. An untried destination is not a broken one, so it is reported
        // reachable — with the sink's own token saying it has simply not been used.
        let c = connectivity_of(&sink, "local", &health);
        assert_eq!(c.instance, "archive");
        assert!(c.connected);
        assert_eq!(c.state.as_deref(), Some("IDLE"));
        assert!(c.detail.as_deref().unwrap().contains("out"), "where the data goes, for a human");
        assert_eq!(c.attributes["destination"], json!("local"), "the open bag carries domain data");

        health.set(DestState::Online);
        assert!(connectivity_of(&sink, "local", &health).connected);
    }

    #[test]
    fn still_retrying_and_gave_up_are_the_same_boolean_and_different_states() {
        // Both are "not connected" to a console's health dot — which is exactly why the normalized
        // flag is not enough on its own, and why `state` carries the sink's own vocabulary.
        let sink = sink_config("archive");
        let health = DestHealth::default();

        health.set(DestState::Backoff);
        let retrying = connectivity_of(&sink, "local", &health);
        health.set(DestState::Failed);
        let gave_up = connectivity_of(&sink, "local", &health);

        assert!(!retrying.connected);
        assert!(!gave_up.connected);
        assert_eq!(retrying.state.as_deref(), Some("BACKOFF"));
        assert_eq!(gave_up.state.as_deref(), Some("FAILED"), "gave up must be loud");
    }

    #[test]
    fn the_key_is_deterministic() {
        use edgecommons::messaging::MessageBuilder;
        let msg = MessageBuilder::new("T", "1.0").payload(json!({})).build();
        let a = key_for("archive", "ecv1/gw/x/main/data/temp", &msg);
        let b = key_for("archive", "ecv1/gw/x/main/data/temp", &msg);
        assert_eq!(a, b, "the same message must always resolve to the same key");
        assert!(a.starts_with("archive/temp/"));
    }
}

/// Apply `component.global.defaults` to a sink that did not set the key itself.
///
/// The schema promises this knob; a knob the code ignores is worse than no knob at all.
fn apply_defaults(sink: &mut SinkConfig, defaults: &serde_json::Value) {
    if sink.max_queue == default_max_queue() {
        if let Some(v) = defaults.get("maxQueue").and_then(serde_json::Value::as_u64) {
            sink.max_queue = usize::try_from(v).unwrap_or_else(|_| default_max_queue());
        }
    }
    let untouched = sink.retry.base_delay_ms == default_base_ms()
        && sink.retry.max_delay_ms == default_max_ms()
        && sink.retry.give_up_after_ms == default_give_up_ms();
    if untouched {
        if let Some(r) = defaults.get("retry") {
            if let Ok(r) = serde_json::from_value::<RetryConfig>(r.clone()) {
                sink.retry = r;
            }
        }
    }
}
