//! # Runtime supervisor — the consume/deliver/retry drivers (the live-infra seam)
//!
//! This is the async **driver** layer: [`App`] wires the `edgecommons` runtime, subscribes each
//! sink's filter, and spawns one delivery task per destination whose loop `.await`s the bounded
//! queue and the destination's live `deliver`/`verify`. It is deliberately kept as thin as possible:
//! every pure decision it composes — the retry backoff ([`RetryConfig::delay`]), the give-up budget
//! ([`RetryConfig::budget_spent`]), the stable delivery key ([`key_for`]), per-destination
//! connectivity ([`connectivity_of`]), and the config defaults ([`apply_defaults`]) — lives in a
//! unit-tested module, not here.
//!
//! Because these functions need a live messaging transport and a live destination to exercise, they
//! are validated by HOST / full-system smoke and the scaffold→build gate, and are excluded from the
//! unit-coverage denominator (`.github/workflows/ci.yml`), exactly as `ethernet-ip-adapter`'s
//! `supervisor.rs`/`publish_sink.rs` seams are. Everything they call stays in the denominator and is
//! tested.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use edgecommons::messaging::Message;
use edgecommons::prelude::*;
use serde_json::json;

use crate::app::{
    apply_defaults, connectivity_of, key_for, DestHealth, DestState, SinkConfig, Stats,
};
use crate::dest::{Item, SharedDestination};

const METRIC_NAME: &str = "sinkDeliveries";

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

/// A cheap uniform `[0, 1)`, so the template needs no `rand` dependency.
fn rand01() -> f64 {
    use std::hash::{BuildHasher, Hasher};
    let n = std::collections::hash_map::RandomState::new().build_hasher().finish();
    (n % 1_000_000) as f64 / 1_000_000.0
}
