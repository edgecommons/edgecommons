//! # Runtime supervisor — the subscribe/route/publish drivers (the live-infra seam)
//!
//! This is the async **driver** layer: [`App`] wires the `edgecommons` runtime, subscribes each
//! route's filters, and spawns one task per route whose select-loop `.await`s the bounded queue, the
//! per-route tick, and `messaging.publish()`. It is deliberately kept as thin as possible: every pure
//! decision it composes — the self-echo guard ([`is_self_echo`]), the config defaults
//! ([`apply_defaults`]), stage construction ([`StageConfig::build`]), and the pipeline mechanics
//! ([`crate::proc`]) — lives in a unit-tested module, not here.
//!
//! Because these functions need a live messaging transport to exercise, they are validated by HOST/
//! full-system smoke and the scaffold→build gate, and are excluded from the unit-coverage denominator
//! (`.github/workflows/ci.yml`), exactly as `ethernet-ip-adapter`'s `supervisor.rs`/`poll_driver.rs`
//! seams are. Everything they call stays in the denominator and is tested.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use edgecommons::messaging::{Message, MessageBuilder};
use edgecommons::prelude::*;
use serde_json::json;

use crate::app::{apply_defaults, is_self_echo, RouteConfig, StageConfig, Stats, Target};
use crate::proc::{Out, Pipeline, ProcMsg};

/// The metric this component emits each interval.
const METRIC_NAME: &str = "processorThroughput";

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

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}
