//! # <<COMPONENTNAME>> — application logic
//!
//! Minimal starting point: holds the `ggcommons` service handles, registers a
//! configuration-change listener (dynamic config pickup), and runs until shutdown.
//! Replace the body of [`App::run`] with your component's business logic.

use std::sync::Arc;

use ggcommons::prelude::*;

/// The component's business logic and the `ggcommons` service handles it operates over.
pub struct App {
    config: Arc<Config>,
    metrics: Arc<dyn MetricService>,
    /// `Some` when a messaging transport is available for the resolved platform
    /// (HOST/MQTT always; GREENGRASS/IPC with the `greengrass` feature).
    messaging: Option<Arc<dyn MessagingService>>,
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
    /// the service handles it needs and registering for config hot-reload.
    pub fn new(gg: &GgCommons) -> anyhow::Result<Self> {
        // Dynamic config pickup: react to deployment/shadow config changes at runtime.
        gg.add_config_change_listener(Arc::new(ConfigListener));

        Ok(Self {
            config: gg.config(),
            metrics: gg.metrics(),
            messaging: gg.messaging().ok(),
        })
    }

    /// Run until a shutdown signal (Ctrl-C / SIGTERM) is received.
    ///
    /// The library owns signal handling (FR-HB-2): await [`GgCommons::shutdown_signal`] rather than
    /// re-implementing `tokio::signal` here, so there is a single signal source. Dropping the
    /// `GgCommons` runtime after this returns releases all resources (RAII).
    pub async fn run(&self, gg: &GgCommons) -> anyhow::Result<()> {
        tracing::info!(identity = %self.config.identity().path(), "<<COMPONENTNAME>> running");

        // TODO: your business logic goes here. The wired services are available as:
        //   - self.messaging  — publish/subscribe + request/reply (Option; see above)
        //   - self.metrics    — self.metrics.define_metric(..) / emit_metric(..)
        //   - self.config     — self.config.global() / self.config.identity()
        //
        // Publish on unified-namespace (UNS) topics minted via `gg.uns()` — never
        // hand-write topics — and build messages `.from_config(..)` so each envelope
        // carries the component identity (config `hierarchy` + `identity` blocks;
        // the last hierarchy level's value is the resolved thing name). E.g.:
        //   let topic = gg.uns().topic_with_channel(UnsClass::Data, "example")?;
        //   let msg = MessageBuilder::new("Example", "1.0")
        //       .from_config(&self.config)
        //       .payload(serde_json::json!({ "value": 42 }))
        //       .build();
        //   if let Some(messaging) = &self.messaging { messaging.publish(&topic, &msg).await?; }
        // For instance-scoped topics/messages use `gg.instance(id)?` instead. The
        // heartbeat (a UNS `state` keepalive) is automatic — on, every 5 s, local —
        // tuned by the optional `heartbeat` config block. The reserved UNS classes
        // (`state`/`metric`/`cfg`/`log`) are library-owned: publishing to them
        // directly is rejected with `GgError::ReservedTopic`.
        //
        // Touch the handles so the starting template compiles without warnings.
        let _ = (&self.metrics, &self.messaging);

        gg.shutdown_signal().await;
        tracing::info!("shutdown signal received; exiting");
        Ok(())
    }
}
