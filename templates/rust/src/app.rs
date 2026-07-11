//! # <<COMPONENTNAME>> — application logic
//!
//! Minimal starting point: holds the `edgecommons` service handles, registers a
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
//! up on the console's Signals/Events/Metrics tabs and something custom to command, instead of
//! an empty dashboard:
//! - a periodic **metric** ([`METRIC_NAME`]: a monotonic `tickCount` counter plus an
//!   `uptimeSecs` gauge-like measure) via `gg.metrics()`;
//! - a periodic **data** signal ([`DATA_SIGNAL_ID`]: a sine-wave demo reading) via `gg.data()` —
//!   the [`DataFacade`] constructs the `SouthboundSignalUpdate` body (device/signal/samples) and
//!   defaults an omitted sample quality to `GOOD`, so the console's Signals tab has something to
//!   chart;
//! - a periodic **evt** (`ecv1/.../evt/info/sample-event`) via `gg.events()` — the
//!   [`EventsFacade`] derives the `evt/{severity}/{type}` channel from the body's own severity +
//!   type, so the topic and body can never disagree;
//! - a custom **command verb** ([`SET_GREETING`]), registered with `gg.commands().register(...)`
//!   alongside the automatic built-ins, that mutates a small piece of in-memory state which the
//!   periodic status publish then reflects on its very next tick — so invoking it from the
//!   console is visibly observable.
//!
//! Replace all four with your own business metrics/signals/events/verbs; none of this is
//! required by the library (a bare scaffold works fine without them), it exists so the
//! demonstrated surface is live end-to-end out of the box.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use edgecommons::messaging::MessageBuilder;
use edgecommons::prelude::*;
use serde_json::json;

/// The demo loop-tick metric name (see the module docs).
const METRIC_NAME: &str = "loopTicks";
/// The demo data() signal id (see the module docs).
const DATA_SIGNAL_ID: &str = "demo-signal";
/// The custom command verb this scaffold registers (see the module docs).
const SET_GREETING: &str = "set-greeting";
/// How often the demo loop ticks (publishes the status/metric/data/evt quartet below).
const TICK_INTERVAL: Duration = Duration::from_secs(10);

/// Shared state installed into the application command before the inbox becomes ACTIVE.
pub type GreetingState = Arc<Mutex<String>>;

/// Construct the demo state before building the EdgeCommons runtime.
pub fn greeting_state() -> GreetingState {
    Arc::new(Mutex::new("Hello from <<COMPONENTNAME>>".to_string()))
}

/// Install every application command before acknowledged inbox startup.
pub fn configure_commands(
    commands: &CommandInbox,
    greeting: GreetingState,
) -> edgecommons::Result<()> {
    commands.register(
        SET_GREETING,
        command_handler(move |request| {
            let greeting = Arc::clone(&greeting);
            async move {
                let next = match request
                    .body
                    .get("greeting")
                    .and_then(|value| value.as_str())
                {
                    Some(value) => value.to_string(),
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
                Ok(Some(
                    json!({ "previousGreeting": previous, "greeting": next }),
                ))
            }
        }),
    )
}

/// The component's business logic and the `edgecommons` service handles it operates over.
pub struct App {
    config: Arc<Config>,
    metrics: Arc<dyn MetricService>,
    /// The `data()` publish facade — bound to the `main` instance (see the module docs).
    data: DataFacade,
    /// The `events()` publish facade — bound to the `main` instance (see the module docs).
    events: EventsFacade,
    /// `Some` when a messaging transport is available for the resolved platform
    /// (HOST/MQTT always; GREENGRASS/IPC with the `greengrass` feature).
    messaging: Option<Arc<dyn MessagingService>>,
    /// The UNS topic builder bound to this component's resolved identity (captured once at
    /// startup, like the Java/Python/TS facades).
    uns: Uns,
    /// In-memory demo state: mutated by the [`SET_GREETING`] command (registered in
    /// [`configure_commands`]), read back by the periodic status publish in [`App::run`] — so a console
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
    /// Build the app from an initialized [`edgecommons::EdgeCommons`] runtime, capturing
    /// the service handles it needs, registering for config hot-reload, defining the demo
    /// metric, and registering the demo custom command verb.
    pub fn new(gg: &EdgeCommons, greeting: GreetingState) -> anyhow::Result<Self> {
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

        Ok(Self {
            config: gg.config(),
            metrics,
            // data()/events() are bound to the `main` instance (== gg.instance("main").data()/
            // .events()); each call mints its own topic from the signal id / severity+type -
            // never hand-write one.
            data: gg.data(),
            events: gg.events(),
            messaging: gg.messaging().ok(),
            uns: gg.uns(),
            greeting,
        })
    }

    /// Run until a shutdown signal (Ctrl-C / SIGTERM) is received, ticking the demo
    /// status/metric/data/evt quartet every [`TICK_INTERVAL`].
    ///
    /// The library owns signal handling (FR-HB-2): `tokio::select!` races the tick timer against
    /// [`EdgeCommons::shutdown_signal`] rather than re-implementing `tokio::signal` here, so there
    /// is a single signal source. Dropping the `EdgeCommons` runtime after this returns releases
    /// all resources (RAII).
    pub async fn run(&self, gg: &EdgeCommons) -> anyhow::Result<()> {
        tracing::info!(identity = %self.config.identity().path(), "<<COMPONENTNAME>> running");

        // Publish on unified-namespace (UNS) topics minted via `self.uns` — never hand-write
        // topics. APP is the free application class for this scaffold's status publish; the
        // data()/events() facades below mint their OWN topics from the signal id / severity+type.
        // For instance-scoped topics/messages use `gg.instance(id)?`.
        let status_topic = self.uns.topic_with_channel(UnsClass::App, "status")?;
        tracing::info!(status = %status_topic, "demo topics minted");

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
                    }

                    // 2) metric - a loop-tick counter plus an uptime-ish gauge (the console's
                    // Metrics tab).
                    let mut values = HashMap::new();
                    values.insert("tickCount".to_string(), seq as f64);
                    values.insert("uptimeSecs".to_string(), uptime_secs as f64);
                    if let Err(e) = self.metrics.emit_metric(METRIC_NAME, values).await {
                        tracing::warn!(error = %e, "metric emit failed");
                    }

                    // 3) data - a periodic sample telemetry signal (the console's Signals tab),
                    // through the data() facade: it constructs the SouthboundSignalUpdate body
                    // (device/signal/samples), sanitizes the channel, and stamps identity - a
                    // real adapter maps one protocol read onto a `Sample` and never touches the
                    // envelope or topic (DESIGN-class-facades §2.1). A sine wave stands in for a
                    // live sensor reading here; `publish_value` with no explicit `Quality`
                    // demonstrates the facade's honest default - an unspecified reading defaults
                    // to `Quality::Good` (marked `qualityRaw:"unspecified"` on the wire so a
                    // consumer can tell a synthesized GOOD from a device-reported one). Use
                    // `publish_value_with_quality`/the `signal(...)` builder when your source
                    // knows a read failed or is stale.
                    let demo_value = 20.0 + 5.0 * ((seq as f64) / 10.0).sin();
                    if let Err(e) = self.data.publish_value(DATA_SIGNAL_ID, demo_value).await {
                        tracing::warn!(error = %e, "data publish failed");
                    }

                    // 4) evt - a discrete, human-meaningful occurrence (not a metric, not
                    // liveness state); the console's Events tab. Through the events() facade:
                    // emit(severity, type, message, context) derives the evt/{severity}/{type}
                    // channel from the body's own severity + type, so the topic and body can
                    // never disagree (DESIGN-class-facades §2.2) - no more hand-built
                    // body/topic. A real component would emit these on actual occurrences (a
                    // threshold crossed, a connection lost/restored, ...), not on a fixed timer;
                    // raise_alarm/clear_alarm are there for stateful alarms.
                    if let Err(e) = self.events.emit(
                        Severity::Info,
                        "sample-event",
                        Some("sample event from <<COMPONENTNAME>>".to_string()),
                        Some(json!({ "seq": seq, "greeting": greeting })),
                    ).await {
                        tracing::warn!(error = %e, "event publish failed");
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
