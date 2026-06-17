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
    /// `Some` when a messaging transport is available for the runtime mode
    /// (STANDALONE always; GREENGRASS with the `greengrass` feature).
    messaging: Option<Arc<dyn MessagingService>>,
}

/// A [`ConfigurationChangeListener`] invoked whenever the component configuration is
/// hot-reloaded (e.g. a Greengrass deployment config change). Put your reaction to
/// config changes here.
struct ConfigListener;

#[async_trait::async_trait]
impl ConfigurationChangeListener for ConfigListener {
    async fn on_configuration_change(&self, config: Arc<Config>) -> bool {
        tracing::info!(thing = %config.thing_name, "configuration changed");
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
    pub async fn run(&self) -> anyhow::Result<()> {
        tracing::info!(thing = %self.config.thing_name, "<<COMPONENTNAME>> running");

        // TODO: your business logic goes here. The wired services are available as:
        //   - self.messaging  — publish/subscribe + request/reply (Option; see above)
        //   - self.metrics    — self.metrics.define_metric(..) / emit_metric(..)
        //   - self.config     — self.config.global() / self.config.thing_name
        // Touch the handles so the starting template compiles without warnings.
        let _ = (&self.metrics, &self.messaging);

        shutdown_signal().await;
        tracing::info!("shutdown signal received; exiting");
        Ok(())
    }
}

/// Resolve when the process receives Ctrl-C or (on Unix) SIGTERM.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
