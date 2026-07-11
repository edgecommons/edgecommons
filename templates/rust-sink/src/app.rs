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
//!   idle. Started / completed / failed / exhausted all go out on the UNS event surface.
//!
//! ## Where the work comes from
//!
//! This scaffold's source is a **subscription**: it consumes messages off the bus and delivers
//! each one. That is the common case. If your source is a watched directory or a polled API,
//! replace [`App::run`]'s subscribe call — everything downstream of `deliver_with_retry` is
//! unchanged, which is the point of the seam.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
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
                Ok(sink) => sinks.push(sink),
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

        for sink in &self.sinks {
            let destination = crate::dest::build(&sink.destination)?;
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
                gg.events(),
            ));
        }

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
    events: EventsFacade,
) {
    while let Some(item) = rx.recv().await {
        deliver_with_retry(&sink, &item, &destination, &stats, &events).await;
    }
    tracing::info!(sink = %sink.id, "sink stopped");
}

/// Deliver one item, retrying transient failures until the time budget is spent.
///
/// The event ladder is the sink's contract with whoever is watching: **started**, then either
/// **completed**, or **failed** (with `willRetry`), and finally **exhausted** if the budget runs
/// out. An operator must be able to tell "still trying" from "gave up", and gave-up must be loud.
async fn deliver_with_retry(
    sink: &SinkConfig,
    item: &Item,
    destination: &SharedDestination,
    stats: &Arc<Stats>,
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
