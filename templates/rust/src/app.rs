//! # <<COMPONENTNAME>> — application logic
//!
//! Minimal starting point: holds the `ggcommons` service handles, registers a
//! configuration-change listener (dynamic config pickup), and runs until shutdown.
//!
//! The `state` heartbeat keepalive AND the component command inbox are both **automatic**
//! (library-owned, no code here): the `state` keepalive publishes on
//! `ecv1/{device}/{component}/main/state` (on / 5 s / local by default), and the inbox
//! (`ecv1/{device}/{component}/main/cmd/#`, `gg.commands()`) already answers `ping` /
//! `reload-config` / `get-configuration` before [`App::new`] even runs.
//!
//! What this scaffold adds is the rest of the monitoring + command surface the edge-console
//! reads (DESIGN-uns §7/§9 — G-S1/S2), so a freshly generated component has something to show
//! up on the console's Events/Metrics tabs and something custom to command, instead of an empty
//! dashboard:
//! - a periodic **metric** ([`METRIC_NAME`]: a monotonic `tickCount` counter plus an
//!   `uptimeSecs` gauge-like measure) via `gg.metrics()`;
//! - a periodic **evt** (`ecv1/.../evt/sample-event`) via the UNS topic builder + `MessageBuilder`
//!   — there is no dedicated `events()` facade yet, so an evt is just a normal published message
//!   on the open `evt` class;
//! - a custom **command verb** ([`SET_GREETING`]), registered with `gg.commands().register(...)`
//!   alongside the automatic built-ins, that mutates a small piece of in-memory state which the
//!   periodic status publish then reflects on its very next tick — so invoking it from the
//!   console is visibly observable.
//!
//! Replace all three with your own business metrics/events/verbs; none of this is required by
//! the library (a bare scaffold works fine without them), it exists so the demonstrated surface
//! is live end-to-end out of the box.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ggcommons::messaging::MessageBuilder;
use ggcommons::prelude::*;
use serde_json::json;

/// The demo loop-tick metric name (see the module docs).
const METRIC_NAME: &str = "loopTicks";
/// The custom command verb this scaffold registers (see the module docs).
const SET_GREETING: &str = "set-greeting";
/// How often the demo loop ticks (publishes the status/evt/metric trio below).
const TICK_INTERVAL: Duration = Duration::from_secs(10);

/// The component's business logic and the `ggcommons` service handles it operates over.
pub struct App {
    config: Arc<Config>,
    metrics: Arc<dyn MetricService>,
    /// `Some` when a messaging transport is available for the resolved platform
    /// (HOST/MQTT always; GREENGRASS/IPC with the `greengrass` feature).
    messaging: Option<Arc<dyn MessagingService>>,
    /// The UNS topic builder bound to this component's resolved identity (captured once at
    /// startup, like the Java/Python/TS facades).
    uns: Uns,
    /// In-memory demo state: mutated by the [`SET_GREETING`] command (registered in
    /// [`App::new`]), read back by the periodic status publish in [`App::run`] — so a console
    /// "Send command" has a visible effect without needing a dedicated custom "get" verb.
    greeting: Arc<Mutex<String>>,
}

/// A [`ConfigurationChangeListener`] invoked whenever the component configuration is
/// hot-reloaded (e.g. a Greengrass deployment config change). Put your reaction to
/// config changes here.
struct ConfigListener;

#[async_trait::async_trait]
impl ConfigurationChangeListener for ConfigListener {
    async fn on_configuration_change(&self, config: Arc<Config>) -> bool {
        tracing::info!(identity = %config.identity().path(), "configuration changed");
        true
    }
}

impl App {
    /// Build the app from an initialized [`ggcommons::GgCommons`] runtime, capturing
    /// the service handles it needs, registering for config hot-reload, defining the demo
    /// metric, and registering the demo custom command verb.
    pub fn new(gg: &GgCommons) -> anyhow::Result<Self> {
        // Dynamic config pickup: react to deployment/shadow config changes at runtime.
        gg.add_config_change_listener(Arc::new(ConfigListener));

        let metrics = gg.metrics();
        // --- metrics: define once, emit every tick in run(). MetricBuilder is the sanctioned
        // construction path (mirrors Java/Python/TS). Two measures show a metric isn't just a
        // single scalar: a monotonic counter (tickCount) and a gauge-like elapsed value
        // (uptimeSecs); add_dimension adds a custom EMF/CloudWatch dimension on top of the
        // library's own default coreName/component dimensions.
        metrics.define_metric(
            MetricBuilder::create(METRIC_NAME)
                .with_config(&gg.config())
                .add_measure("tickCount", "Count", 60)
                .add_measure("uptimeSecs", "Seconds", 60)
                .add_dimension("demo", "scaffold")
                .build(),
        );

        let greeting = Arc::new(Mutex::new("Hello from <<COMPONENTNAME>>".to_string()));

        // --- commands: ping/reload-config/get-configuration are already live (wired by
        // GgCommonsBuilder::build before this runs). Register ONE custom verb so there is
        // something for the console's "Send command" to invoke beyond the built-ins.
        // `gg.commands()` is only `None` when no messaging transport was wired at all.
        if let Some(commands) = gg.commands() {
            let greeting_for_handler = Arc::clone(&greeting);
            commands.register(
                SET_GREETING,
                command_handler(move |request| {
                    let greeting = Arc::clone(&greeting_for_handler);
                    async move {
                        let next = match request.body.get("greeting").and_then(|v| v.as_str()) {
                            Some(g) => g.to_string(),
                            None => {
                                return Err(CommandError::new(
                                    "BAD_ARGS",
                                    "expected a JSON body {\"greeting\": \"<text>\"}",
                                ));
                            }
                        };
                        let previous = {
                            let mut guard = greeting.lock().expect("greeting mutex poisoned");
                            std::mem::replace(&mut *guard, next.clone())
                        };
                        Ok(Some(json!({ "previousGreeting": previous, "greeting": next })))
                    }
                }),
            )?;
        }

        Ok(Self {
            config: gg.config(),
            metrics,
            messaging: gg.messaging().ok(),
            uns: gg.uns(),
            greeting,
        })
    }

    /// Run until a shutdown signal (Ctrl-C / SIGTERM) is received, ticking the demo
    /// status/metric/evt trio every [`TICK_INTERVAL`].
    ///
    /// The library owns signal handling (FR-HB-2): `tokio::select!` races the tick timer against
    /// [`GgCommons::shutdown_signal`] rather than re-implementing `tokio::signal` here, so there
    /// is a single signal source. Dropping the `GgCommons` runtime after this returns releases
    /// all resources (RAII).
    pub async fn run(&self, gg: &GgCommons) -> anyhow::Result<()> {
        tracing::info!(identity = %self.config.identity().path(), "<<COMPONENTNAME>> running");

        // Publish on unified-namespace (UNS) topics minted via `self.uns` — never hand-write
        // topics. APP is the free application class; EVT is for discrete, notable occurrences
        // (this scaffold's sample event) — metric publishes go through `self.metrics` above,
        // never a hand-built topic. For instance-scoped topics/messages use `gg.instance(id)?`.
        let status_topic = self.uns.topic_with_channel(UnsClass::App, "status")?;
        let event_topic = self.uns.topic_with_channel(UnsClass::Evt, "sample-event")?;
        tracing::info!(status = %status_topic, event = %event_topic, "demo topics minted");

        let mut ticker = tokio::time::interval(TICK_INTERVAL);
        let start = Instant::now();
        let mut seq: u64 = 0;
        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    seq += 1;
                    let uptime_secs = start.elapsed().as_secs();
                    let greeting = self.greeting.lock().expect("greeting mutex poisoned").clone();

                    if let Some(messaging) = &self.messaging {
                        // 1) app status - reflects the current greeting (mutable via the
                        // set-greeting command above), so a console operator can watch a
                        // command's effect land on the next tick.
                        let status_msg = MessageBuilder::new("StatusUpdate", "1.0")
                            .from_config(&self.config)
                            .payload(json!({ "seq": seq, "message": greeting }))
                            .build();
                        if let Err(e) = messaging.publish(&status_topic, &status_msg).await {
                            tracing::warn!(error = %e, "status publish failed");
                        }

                        // 3) evt - a discrete, human-meaningful occurrence (not a metric, not
                        // liveness state); the console's Events tab. A real component would emit
                        // these on actual occurrences (a threshold crossed, a connection
                        // lost/restored, ...), not on a fixed timer.
                        let event_msg = MessageBuilder::new("SampleEvent", "1.0")
                            .from_config(&self.config)
                            .payload(json!({
                                "severity": "info",
                                "message": "sample event from <<COMPONENTNAME>>",
                                "context": { "seq": seq, "greeting": greeting },
                            }))
                            .build();
                        if let Err(e) = messaging.publish(&event_topic, &event_msg).await {
                            tracing::warn!(error = %e, "event publish failed");
                        }
                    }

                    // 2) metric - a loop-tick counter plus an uptime-ish gauge (the console's
                    // Metrics tab).
                    let mut values = HashMap::new();
                    values.insert("tickCount".to_string(), seq as f64);
                    values.insert("uptimeSecs".to_string(), uptime_secs as f64);
                    if let Err(e) = self.metrics.emit_metric(METRIC_NAME, values).await {
                        tracing::warn!(error = %e, "metric emit failed");
                    }

                    tracing::info!(seq, uptime_secs, %greeting, "tick");
                }
                _ = gg.shutdown_signal() => {
                    tracing::info!("shutdown signal received; exiting");
                    break;
                }
            }
        }
        Ok(())
    }
}
