//! # <<COMPONENTNAME>> — a processing component
//!
//! A **processor** subscribes to messages, transforms them, and forwards the result. This
//! scaffold wires that shape end to end; the transformation itself lives in [`crate::proc`],
//! which is where your code goes.
//!
//! ```text
//!   subscribe(filter) ──► bounded queue ──► one task per route ──► publish
//!                                              (Pipeline)          local | northbound
//! ```
//!
//! Each entry of `component.instances[]` is **one route**: topic filters, a pipeline of stages,
//! and a target. Routes are independent — one task each — so a slow route cannot stall another,
//! and the per-key state inside a stage needs no lock.
//!
//! ## Why a processor uses `messaging()` and not `data()`
//!
//! Worth reading twice, because it is the mistake this archetype invites. The `data()` facade is
//! for a component that *produces* readings: it mints its own topic from a signal id and imposes
//! the `SouthboundSignalUpdate` body. A processor is **payload-agnostic** — it republishes what
//! it was handed, on a topic its route names. Routing that through `data()` would rewrite both
//! the topic and the body, which is exactly what a republisher must not do. So: raw
//! `gg.messaging()`, and topics from config.
//!
//! ## Two guards that are not optional
//!
//! * **Self-echo.** A processor that publishes onto a class it also subscribes to will consume
//!   its own output, reprocess it, republish it, and saturate the device. [`is_self_echo`] drops
//!   anything carrying our own identity.
//! * **Identity restamp.** What we publish is *ours*. Without the restamp the fleet cannot tell
//!   who emitted a message — and the self-echo guard downstream cannot work either.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use edgecommons::messaging::{Message, MessageBuilder};
use edgecommons::prelude::*;
use serde::Deserialize;
use serde_json::json;

use crate::proc::{CountPerTick, FieldEquals, Out, Pipeline, ProcMsg, Processor};

/// The metric this component emits each interval.
const METRIC_NAME: &str = "processorThroughput";

/// Where a route's output goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Target {
    /// The device-local bus — the common case; another component on this device consumes it.
    #[default]
    Local,
    /// Straight out to the northbound broker.
    Northbound,
}

/// One route == one entry of `component.instances[]`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RouteConfig {
    pub id: String,
    /// Topic filters to subscribe to. Wildcards are fine: `ecv1/+/+/+/data/#`.
    #[serde(default)]
    pub subscribe: Vec<String>,
    /// The topic the result is published on.
    pub publish_topic: String,
    #[serde(default)]
    pub target: Target,
    /// The stages, in order. An empty pipeline is a pass-through republisher.
    #[serde(default)]
    pub pipeline: Vec<StageConfig>,
    /// How many messages may be queued for this route before new ones are dropped.
    ///
    /// Bounded on purpose. An unbounded queue does not remove backpressure — it relocates the
    /// failure to the heap, and by the time you notice you have lost the ability to report it.
    #[serde(default = "default_max_queue")]
    pub max_queue: usize,
    /// How often stateful stages are ticked, in milliseconds.
    #[serde(default = "default_tick_ms")]
    pub tick_ms: u64,
}

fn default_max_queue() -> usize {
    256
}
fn default_tick_ms() -> u64 {
    10_000
}

/// A stage, as named in config. Add a variant here as you add a stage.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StageConfig {
    /// Keep only messages whose dotted body path equals a value.
    FieldEquals { path: String, value: serde_json::Value },
    /// Accumulate arrivals; emit a rollup every tick.
    ///
    /// A struct variant with no fields, not a unit variant: `{"countPerTick": {}}` is what the
    /// config and the schema say, and a unit variant would demand the bare string instead.
    CountPerTick {},
}

impl StageConfig {
    fn build(&self) -> Box<dyn Processor> {
        match self {
            Self::FieldEquals { path, value } => {
                Box::new(FieldEquals { path: path.clone(), value: value.clone() })
            }
            Self::CountPerTick {} => Box::new(CountPerTick { seen: 0, last: None }),
        }
    }
}

/// Counters, reported as a metric each interval.
#[derive(Default)]
pub struct Stats {
    pub received: AtomicU64,
    pub published: AtomicU64,
    /// Dropped because a route's queue was full. **Never let this be invisible** — a processor
    /// that silently discards messages is worse than one that crashes.
    pub dropped: AtomicU64,
    pub errors: AtomicU64,
}

pub struct App {
    config: Arc<Config>,
    metrics: Arc<dyn MetricService>,
    routes: Vec<RouteConfig>,
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
                .add_measure("published", "Count", 60)
                .add_measure("dropped", "Count", 60)
                .add_measure("errors", "Count", 60)
                .build(),
        );

        // One route per instance. A malformed route is skipped with a warning rather than killing
        // the component — but if *every* route is malformed there is nothing to run, and failing
        // loudly beats idling silently.
        let mut routes = Vec::new();
        for id in config.instance_ids() {
            match config
                .instance(&id)
                .ok_or_else(|| anyhow::anyhow!("no config"))
                .and_then(|v| Ok(serde_json::from_value::<RouteConfig>(v.clone())?))
            {
                Ok(mut route) => {
                    apply_defaults(&mut route, &defaults);
                    routes.push(route);
                }
                Err(e) => tracing::warn!("skipping malformed route `{id}`: {e}"),
            }
        }
        anyhow::ensure!(!routes.is_empty(), "no valid routes in component.instances[]");

        // ONE provider, TWO surfaces: whatever it returns is pushed into the `state` keepalive's
        // `instances[]` on every tick AND returned by the built-in `status` command verb when a
        // console asks. Whoever watches and whoever asks cannot get different answers.
        //
        // A processor owns no southbound links — its routes are message flows, not connections —
        // so it reports NO instances. That is a real answer, not a missing one: with an empty vec
        // the `instances[]` section is omitted and `status` says exactly what `ping` says. Register
        // it anyway, so the seam is visible the day this component grows a connection of its own.
        //
        // When it does (an enrichment database, a model server), return one entry per connection:
        //
        //     InstanceConnectivity::of(&id, db.is_connected())      // the NORMALIZED flag: always
        //         .with_state("ONLINE")                             // present, so any console can
        //         .with_attributes(attributes)                      // render a health dot without
        //                                                           // knowing this component
        //
        // `state` is your own vocabulary (ONLINE / CONNECTING / BACKOFF / DISABLED — a boolean
        // cannot tell "reconnecting" from "administratively off"); `attributes` is the open bag for
        // domain data, deliberately unconstrained so it never destabilizes the fields above.
        let no_instances: Arc<InstanceConnectivityProvider> = Arc::new(Vec::new);
        gg.set_instance_connectivity_provider(Some(no_instances));

        Ok(Self { config, metrics, routes, stats: Arc::new(Stats::default()) })
    }

    pub async fn run(&self, gg: &EdgeCommons) -> anyhow::Result<()> {
        let Ok(messaging) = gg.messaging() else {
            anyhow::bail!("a processor needs a messaging transport, and none was wired");
        };

        // Our own identity, captured once: the self-echo guard compares against it per message.
        let me = (
            self.config.identity().path().to_string(),
            self.config.identity().component().to_string(),
        );

        for route in &self.routes {
            let (tx, rx) = tokio::sync::mpsc::channel::<ProcMsg>(route.max_queue);

            for filter in &route.subscribe {
                let tx = tx.clone();
                let stats = Arc::clone(&self.stats);
                let me = me.clone();
                messaging
                    .subscribe(
                        filter,
                        message_handler(move |topic: String, msg: Message| {
                            let tx = tx.clone();
                            let stats = Arc::clone(&stats);
                            let me = me.clone();
                            async move {
                                if is_self_echo(&msg, &me.0, &me.1) {
                                    return; // our own output; consuming it would loop forever
                                }
                                stats.received.fetch_add(1, Ordering::Relaxed);
                                // try_send, never send: a full queue must DROP and be COUNTED,
                                // not block the transport's dispatch task.
                                if tx.try_send(ProcMsg::new(topic, msg)).is_err() {
                                    stats.dropped.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }),
                        route.max_queue,
                        1,
                    )
                    .await?;
                tracing::info!(route = %route.id, filter = %filter, "subscribed");
            }

            tokio::spawn(run_route(
                route.clone(),
                rx,
                Arc::clone(&messaging),
                Arc::clone(&self.config),
                Arc::clone(&self.stats),
                gg.events(),
            ));
        }

        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(60));
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
        let mut values = HashMap::new();
        values.insert("received".to_string(), self.stats.received.swap(0, Ordering::Relaxed) as f64);
        values.insert("published".to_string(), self.stats.published.swap(0, Ordering::Relaxed) as f64);
        values.insert("dropped".to_string(), self.stats.dropped.swap(0, Ordering::Relaxed) as f64);
        values.insert("errors".to_string(), self.stats.errors.swap(0, Ordering::Relaxed) as f64);
        if let Err(e) = self.metrics.emit_metric(METRIC_NAME, values).await {
            tracing::warn!(error = %e, "metric emit failed");
        }
    }
}

/// One route's task. Three arms, and they are the archetype:
/// a message arrived → run the pipeline; the tick fired → let stateful stages emit;
/// the queue closed → drain once more and stop.
async fn run_route(
    route: RouteConfig,
    mut rx: tokio::sync::mpsc::Receiver<ProcMsg>,
    messaging: Arc<dyn MessagingService>,
    config: Arc<Config>,
    stats: Arc<Stats>,
    events: EventsFacade,
) {
    let mut pipeline = Pipeline::new(route.pipeline.iter().map(StageConfig::build).collect());
    let mut ticker = tokio::time::interval(std::time::Duration::from_millis(route.tick_ms));

    loop {
        let out = tokio::select! {
            got = rx.recv() => match got {
                Some(m) => pipeline.run(smallvec::smallvec![m], None),
                None => break, // channel closed: shutting down
            },
            _ = ticker.tick() => pipeline.run(Out::new(), Some(now_ms())),
        };
        dispatch(&route, out, &messaging, &config, &stats, &events).await;
    }

    // A final tick on the way out, so a half-full window is emitted rather than silently lost.
    let out = pipeline.run(Out::new(), Some(u64::MAX));
    dispatch(&route, out, &messaging, &config, &stats, &events).await;
    tracing::info!(route = %route.id, "route stopped");
}

async fn dispatch(
    route: &RouteConfig,
    out: Out,
    messaging: &Arc<dyn MessagingService>,
    config: &Arc<Config>,
    stats: &Arc<Stats>,
    events: &EventsFacade,
) {
    for m in out {
        // Restamp identity: what we publish is OURS, not the producer's.
        let msg = MessageBuilder::new(&m.msg.header.name, &m.msg.header.version)
            .from_config(config)
            .payload(m.msg.body.clone())
            .build();

        let result = match route.target {
            Target::Local => messaging.publish(&route.publish_topic, &msg).await,
            Target::Northbound => messaging.publish_northbound(&route.publish_topic, &msg, Qos::AtLeastOnce).await,
        };

        if let Err(e) = result {
            stats.errors.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(route = %route.id, error = %e, "publish failed");
            let _ = events
                .emit(
                    Severity::Warning,
                    "publish-failed",
                    Some(format!("route {} could not publish", route.id)),
                    Some(json!({ "route": route.id, "topic": route.publish_topic })),
                )
                .await;
        } else {
            stats.published.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Would consuming this message mean consuming our own output?
#[must_use]
pub fn is_self_echo(msg: &Message, my_path: &str, my_component: &str) -> bool {
    msg.identity
        .as_ref()
        .is_some_and(|id| id.path() == my_path && id.component() == my_component)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_route_parses_from_its_instance_config() {
        let route: RouteConfig = serde_json::from_value(json!({
            "id": "temps",
            "subscribe": ["ecv1/+/+/+/data/#"],
            "publishTopic": "ecv1/gw01/<<BINNAME>>/main/data/rollup",
            "target": "northbound",
            "pipeline": [
                { "fieldEquals": { "path": "signal.id", "value": "temp-1" } },
                { "countPerTick": {} }
            ],
            "tickMs": 5000
        }))
        .unwrap();

        assert_eq!(route.id, "temps");
        assert_eq!(route.target, Target::Northbound);
        assert_eq!(route.pipeline.len(), 2);
        assert_eq!(route.tick_ms, 5_000);
        assert_eq!(route.max_queue, 256, "the queue is bounded by default");
    }

    #[test]
    fn the_defaults_are_the_common_case() {
        let route: RouteConfig =
            serde_json::from_value(json!({ "id": "r", "publishTopic": "t" })).unwrap();
        assert_eq!(route.target, Target::Local, "the device-local bus is the common target");
        assert!(route.pipeline.is_empty(), "no stages == a pass-through republisher");
    }

    #[test]
    fn an_unknown_config_key_is_rejected_rather_than_ignored() {
        // deny_unknown_fields: a typo'd route key is a mistake, not a no-op.
        let bad = serde_json::from_value::<RouteConfig>(json!({
            "id": "r", "publishTopic": "t", "pipelnie": []
        }));
        assert!(bad.is_err());
    }
}

/// Apply `component.global.defaults` to a route that did not set the key itself.
///
/// The schema promises this knob; a knob the code ignores is worse than no knob at all.
fn apply_defaults(route: &mut RouteConfig, defaults: &serde_json::Value) {
    if route.tick_ms == default_tick_ms() {
        if let Some(v) = defaults.get("tickMs").and_then(serde_json::Value::as_u64) {
            route.tick_ms = v;
        }
    }
    if route.max_queue == default_max_queue() {
        if let Some(v) = defaults.get("maxQueue").and_then(serde_json::Value::as_u64) {
            route.max_queue = usize::try_from(v).unwrap_or_else(|_| default_max_queue());
        }
    }
}
