//! # GGCommons (Rust)
//!
//! Rust implementation of the Greengrass Commons library — a third implementation
//! alongside the Java (canonical) and Python libraries. It bundles the
//! cross-cutting concerns of an AWS IoT Greengrass v2 component (configuration,
//! messaging, metrics, heartbeat, logging) behind service traits so component
//! authors write only business logic.
//!
//! **Status:** complete and validated on-device. The STANDALONE runtime, the
//! cross-language parity work, and Greengrass IPC (the `greengrass` feature: IPC
//! messaging, `GG_CONFIG`, `SHADOW`, and `CONFIG_COMPONENT`) are all implemented and
//! have been **validated against a live Greengrass core** (non-root), including the
//! real-time device-shadow round-trip. See `../GGCOMMONS_RUST_PORT.md` for the full
//! design and history.
//!
//! ```no_run
//! use ggcommons::prelude::*;
//!
//! # async fn run() -> ggcommons::Result<()> {
//! let gg = GgCommonsBuilder::new("com.example.MyComponent")
//!     .args(std::env::args_os())
//!     .build()
//!     .await?;
//!
//! let cfg = gg.config();
//! println!("component {} on thing {}", gg.component_name(), cfg.thing_name);
//! # Ok(())
//! # }
//! ```

pub mod cli;
pub mod config;
pub mod error;
pub mod heartbeat;
#[cfg(feature = "greengrass")]
pub mod ipc;
pub mod logging;
pub mod messaging;
pub mod metrics;
#[cfg(feature = "credentials")]
pub mod credentials;
#[cfg(feature = "streaming")]
pub mod streaming;

#[cfg(test)]
mod testutil;

pub use error::{GgError, Result};

use std::ffi::OsString;
use std::sync::Arc;

use arc_swap::ArcSwap;

use crate::cli::ParsedArgs;
use crate::config::model::Config;

/// Default thing name when none is supplied and not running under Greengrass.
const DEFAULT_THING_NAME: &str = "NOT_GREENGRASS";
/// Greengrass-injected environment variable for the core's thing name.
const THING_NAME_ENV: &str = "AWS_IOT_THING_NAME";

/// The initialized component runtime. Holds the wired services and the current
/// configuration snapshot. Dropping it releases owned resources (RAII) — there is
/// no separate `close()` to forget.
pub struct GgCommons {
    component_name: String,
    args: ParsedArgs,
    config: Arc<ArcSwap<Config>>,
    messaging: Option<Arc<dyn messaging::MessagingService>>,
    metrics: Arc<dyn metrics::MetricService>,
    /// Telemetry streams (the `streaming` feature). Always present when the feature is on;
    /// empty if no `streaming` config section was provided.
    #[cfg(feature = "streaming")]
    streams: Arc<dyn streaming::StreamService>,
    /// Owns the streaming stats→metrics task; dropping `GgCommons` stops it (RAII).
    #[cfg(feature = "streaming")]
    _stream_metrics: Option<streaming::StreamMetricsBridge>,
    /// Credential service (the `credentials` feature). `None` when the component config has no
    /// `credentials` section.
    #[cfg(feature = "credentials")]
    credentials: Option<Arc<dyn credentials::CredentialService>>,
    /// Config-change listeners notified on hot reload.
    listeners: ConfigListeners,
    /// Owns the heartbeat task; dropping `GgCommons` stops it (RAII).
    _heartbeat: heartbeat::Heartbeat,
    /// Owns the hot-reload task; aborted on drop. `None` if the source can't watch.
    _reload_task: Option<AbortOnDrop>,
    /// Keeps the config source (and its OS file watcher) alive for hot reload.
    _config_source: Box<dyn config::source::ConfigSource>,
}

/// Shared, mutable set of config-change listeners.
type ConfigListeners = Arc<std::sync::Mutex<Vec<Arc<dyn config::ConfigurationChangeListener>>>>;

/// Aborts a background task when dropped (RAII).
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl GgCommons {
    /// The component's full name.
    pub fn component_name(&self) -> &str {
        &self.component_name
    }

    /// The parsed standard CLI arguments.
    pub fn args(&self) -> &ParsedArgs {
        &self.args
    }

    /// A consistent snapshot of the current configuration. Cheap to call; returns
    /// the live snapshot, which is replaced atomically on hot reload.
    pub fn config(&self) -> Arc<Config> {
        self.config.load_full()
    }

    /// The messaging service for this component.
    ///
    /// # Purpose
    /// Obtain the wired [`messaging::MessagingService`] (the testable seam) for
    /// publish/subscribe and request/reply.
    ///
    /// # Errors
    /// | Error Variant | Condition | Recovery |
    /// |---------------|-----------|----------|
    /// | `GgError::Messaging` | No messaging service was wired (GREENGRASS mode without the `greengrass` feature) | Enable the `greengrass` feature, or run in STANDALONE mode |
    pub fn messaging(&self) -> Result<Arc<dyn messaging::MessagingService>> {
        self.messaging.clone().ok_or_else(|| {
            GgError::Messaging(
                "messaging is not available: GREENGRASS mode requires the 'greengrass' feature, \
                 or use STANDALONE mode"
                    .to_string(),
            )
        })
    }

    /// The metric service for this component (the testable seam).
    pub fn metrics(&self) -> Arc<dyn metrics::MetricService> {
        self.metrics.clone()
    }

    /// The telemetry-streaming service for this component (the `streaming` feature).
    ///
    /// Returns the wired [`streaming::StreamService`]; obtain a stream with
    /// [`streaming::StreamService::stream`]. The service is empty (no streams) unless the
    /// component config has a `streaming` section.
    #[cfg(feature = "streaming")]
    pub fn streams(&self) -> Arc<dyn streaming::StreamService> {
        self.streams.clone()
    }

    /// The credential service for this component (the `credentials` feature), or `None` when the
    /// config has no `credentials` section. Mirrors Java/TS `getCredentials()` / Python
    /// `get_credentials()`.
    #[cfg(feature = "credentials")]
    pub fn credentials(&self) -> Option<Arc<dyn credentials::CredentialService>> {
        self.credentials.clone()
    }

    /// Register a listener invoked after the configuration is hot-reloaded.
    ///
    /// Mirrors the Java/Python `addConfigurationChangeListener`. The listener fires on
    /// successful reloads of a watchable config source (e.g. `FILE`).
    pub fn add_config_change_listener(&self, listener: Arc<dyn config::ConfigurationChangeListener>) {
        if let Ok(mut listeners) = self.listeners.lock() {
            listeners.push(listener);
        }
    }

    /// Remove a previously-registered config-change listener (by identity).
    pub fn remove_config_change_listener(&self, listener: &Arc<dyn config::ConfigurationChangeListener>) {
        if let Ok(mut listeners) = self.listeners.lock() {
            listeners.retain(|existing| !Arc::ptr_eq(existing, listener));
        }
    }
}

/// Fluent builder for [`GgCommons`] (the supported construction path).
pub struct GgCommonsBuilder {
    component_name: String,
    argv: Option<Vec<OsString>>,
    receive_own_messages: bool,
}

impl GgCommonsBuilder {
    /// Start building a component runtime with the given full component name.
    pub fn new(component_name: impl Into<String>) -> Self {
        Self {
            component_name: component_name.into(),
            argv: None,
            // Default matches Java/Python (`receiveOwnMessages = true`).
            receive_own_messages: true,
        }
    }

    /// Whether the component should receive messages it itself published (mirrors the
    /// Java/Python `receiveOwnMessages` flag; default `true`).
    ///
    /// **Limitation:** setting this to `false` is currently a **no-op**. The
    /// underlying `aws-greengrass-component-sdk` does not expose the Greengrass IPC
    /// `SubscribeToTopic` `ReceiveMode` (`RECEIVE_MESSAGES_FROM_OTHERS`) that the
    /// Java/Python libraries use, so own-message suppression cannot be performed
    /// natively, and a client-side equivalent cannot reliably cover all message
    /// shapes (e.g. raw messages carry no header/tags to identify the sender). When
    /// `false` is requested, [`build`](Self::build) logs a warning and proceeds as if
    /// `true`. The flag is retained for API parity and forward-compatibility; see the
    /// upstream feature request to add `ReceiveMode` to the SDK.
    pub fn receive_own_messages(mut self, receive_own_messages: bool) -> Self {
        self.receive_own_messages = receive_own_messages;
        self
    }

    /// Supply the argv (including the program name, as from `std::env::args_os()`).
    /// If not set, the process arguments are used.
    pub fn args<I, T>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString>,
    {
        self.argv = Some(args.into_iter().map(Into::into).collect());
        self
    }

    /// Parse arguments, load and validate configuration, initialize logging,
    /// messaging, metrics, and heartbeat, and return the runtime.
    pub async fn build(self) -> Result<GgCommons> {
        let parsed = match self.argv {
            Some(argv) => cli::parse_from(argv)?,
            None => cli::parse_from(std::env::args_os())?,
        };

        let thing_name = parsed.thing.clone().unwrap_or_else(|| {
            std::env::var(THING_NAME_ENV).unwrap_or_else(|_| DEFAULT_THING_NAME.to_string())
        });

        // Messaging is initialized first: it depends only on the runtime mode (the
        // `-m` messaging config / IPC), not on the component config — and the
        // CONFIG_COMPONENT source needs a messaging handle to fetch the config.
        let messaging = init_messaging(&parsed.mode).await?;

        let source = config::source::build(
            &parsed.config,
            messaging.clone(),
            &thing_name,
            &self.component_name,
        )?;
        let raw = source.load().await?;
        config::validation::validate(&raw)?;
        let cfg = Config::from_value(self.component_name.clone(), thing_name.clone(), raw)?;

        logging::init(&cfg);

        // Option C: the SDK exposes no IPC ReceiveMode, so `receiveOwnMessages=false`
        // is a documented no-op rather than a silently-broken or memory-unbounded
        // client-side filter. Warn so the developer is not surprised.
        if !self.receive_own_messages {
            tracing::warn!(
                "receiveOwnMessages=false is not supported by the Greengrass Rust SDK \
                 (no IPC ReceiveMode); proceeding as if true — the component WILL receive \
                 its own messages on subscribed topics"
            );
        }

        tracing::info!(
            component = %self.component_name,
            thing = %cfg.thing_name,
            config_source = source.source_name(),
            "GGCommons initialized"
        );

        let config: Arc<ArcSwap<Config>> = Arc::new(ArcSwap::from_pointee(cfg));
        let snapshot = config.load_full();
        let emitter = Arc::new(metrics::MetricEmitter::new(&snapshot, messaging.clone()).await?);
        let metrics: Arc<dyn metrics::MetricService> = emitter.clone();
        let heartbeat = heartbeat::Heartbeat::start(config.clone(), metrics.clone(), messaging.clone());

        // Credentials / local vault (feature-gated): open the shared vault when the config has a
        // `credentials` section, resolving path templates ({ThingName}/{ComponentFullName}) first.
        // Opened before streaming so the streaming config can reference vault secrets. `None` when
        // no section is present.
        #[cfg(feature = "credentials")]
        let credentials: Option<Arc<dyn credentials::CredentialService>> =
            match snapshot.raw.get("credentials") {
                None => None,
                Some(value) => {
                    let mut cfg: credentials::CredentialsConfig = serde_json::from_value(value.clone())?;
                    cfg.vault.path = config::template::resolve(&snapshot, &cfg.vault.path);
                    if let Some(kp) = cfg.vault.key_provider.key_path.as_mut() {
                        *kp = config::template::resolve(&snapshot, kp);
                    }
                    // Transparently namespace every key by <thingName>/<componentName> so a shared
                    // device vault / fleet central store can't collide across components or devices.
                    let namespace = format!("{}/{}", snapshot.thing_name, self.component_name);
                    let svc = credentials::open_namespaced(&cfg, &namespace)?;
                    Some(Arc::new(svc) as Arc<dyn credentials::CredentialService>)
                }
            };

        // Telemetry streaming (feature-gated): open/recover configured streams and bridge their
        // stats into the metric targets. Empty + no bridge when no `streaming` section exists.
        // `{"$secret": ...}` refs in the streaming config are resolved from the vault first (closes
        // TELEMETRY_STREAMING.md §7) without mutating the public config snapshot.
        #[cfg(feature = "streaming")]
        let (streams, stream_metrics) = {
            #[cfg(feature = "credentials")]
            let resolved_cfg: Option<Config> = match (&credentials, snapshot.raw.get("streaming")) {
                (Some(creds), Some(_)) => {
                    let mut c = (*snapshot).clone();
                    if let Some(s) = c.raw.get_mut("streaming") {
                        credentials::resolve_secret_refs(s, creds.as_ref())?;
                    }
                    Some(c)
                }
                _ => None,
            };
            #[cfg(feature = "credentials")]
            let cfg_ref: &Config = resolved_cfg.as_ref().unwrap_or_else(|| snapshot.as_ref());
            #[cfg(not(feature = "credentials"))]
            let cfg_ref: &Config = snapshot.as_ref();

            let svc: Arc<dyn streaming::StreamService> =
                Arc::new(streaming::DefaultStreamService::open(cfg_ref)?);
            let bridge = streaming::StreamMetricsBridge::start(svc.clone(), metrics.clone());
            (svc, bridge)
        };

        // Internal listeners reconfigure the metric target and logging on hot reload.
        let listeners: ConfigListeners = Arc::new(std::sync::Mutex::new(Vec::new()));
        if let Ok(mut l) = listeners.lock() {
            l.push(emitter as Arc<dyn config::ConfigurationChangeListener>);
            l.push(Arc::new(logging::LoggingReconfigurer) as Arc<dyn config::ConfigurationChangeListener>);
        }

        let reload_task = source.watch().map(|updates| {
            spawn_config_reload(
                updates,
                config.clone(),
                listeners.clone(),
                self.component_name.clone(),
                thing_name,
            )
        });

        Ok(GgCommons {
            component_name: self.component_name,
            args: parsed,
            config,
            messaging,
            metrics,
            #[cfg(feature = "streaming")]
            streams,
            #[cfg(feature = "streaming")]
            _stream_metrics: stream_metrics,
            #[cfg(feature = "credentials")]
            credentials,
            listeners,
            _heartbeat: heartbeat,
            _reload_task: reload_task,
            _config_source: source,
        })
    }
}

/// Spawn the task that applies hot-reloaded config documents: validate, publish a
/// new snapshot atomically, then notify listeners.
fn spawn_config_reload(
    mut updates: tokio::sync::mpsc::UnboundedReceiver<serde_json::Value>,
    config: Arc<ArcSwap<Config>>,
    listeners: ConfigListeners,
    component_name: String,
    thing_name: String,
) -> AbortOnDrop {
    AbortOnDrop(tokio::spawn(async move {
        while let Some(raw) = updates.recv().await {
            if let Err(e) = config::validation::validate(&raw) {
                tracing::warn!(error = %e, "reloaded config failed validation; keeping previous");
                continue;
            }
            match Config::from_value(component_name.clone(), thing_name.clone(), raw) {
                Ok(new_config) => {
                    let snapshot = Arc::new(new_config);
                    config.store(snapshot.clone());
                    tracing::info!("configuration reloaded");
                    let current = listeners.lock().map(|l| l.clone()).unwrap_or_default();
                    for listener in current {
                        let _ = listener.on_configuration_change(snapshot.clone()).await;
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "reloaded config could not be parsed; keeping previous")
                }
            }
        }
    }))
}

/// Initialize the messaging service for the selected runtime mode.
///
/// # Purpose
/// In STANDALONE mode, load the messaging config and connect the dual-broker MQTT
/// provider; in GREENGRASS mode, connect the Greengrass IPC provider (requires the
/// `greengrass` feature; returns `None` only if that feature is disabled).
///
/// # Semantics & Syntax
/// - **Signature**: `async fn init_messaging(mode: &RuntimeMode) -> Result<Option<Arc<dyn MessagingService>>>`
///
/// # Errors
/// | Error Variant | Condition | Recovery |
/// |---------------|-----------|----------|
/// | `GgError::Io` / `GgError::Json` | Messaging config file missing or malformed | Check the `-m STANDALONE <path>` file |
/// | `GgError::Messaging` | Broker connection failed, or `standalone` feature disabled | Verify the broker; enable the feature |
async fn init_messaging(
    mode: &cli::RuntimeMode,
) -> Result<Option<Arc<dyn messaging::MessagingService>>> {
    match mode {
        cli::RuntimeMode::Standalone {
            messaging_config_path,
        } => {
            #[cfg(feature = "standalone")]
            {
                use crate::messaging::config::MessagingConfig;
                use crate::messaging::provider::mqtt::MqttProvider;
                use crate::messaging::service::DefaultMessagingService;

                let mc = MessagingConfig::load(messaging_config_path).await?;
                let provider = Arc::new(MqttProvider::connect(&mc).await?);
                let service: Arc<dyn messaging::MessagingService> =
                    Arc::new(DefaultMessagingService::new(provider));
                Ok(Some(service))
            }
            #[cfg(not(feature = "standalone"))]
            {
                let _ = messaging_config_path;
                Err(GgError::Messaging(
                    "STANDALONE messaging requires the 'standalone' cargo feature".to_string(),
                ))
            }
        }
        cli::RuntimeMode::Greengrass => {
            #[cfg(feature = "greengrass")]
            {
                use crate::messaging::provider::ipc::IpcProvider;
                use crate::messaging::service::DefaultMessagingService;

                let provider = Arc::new(IpcProvider::connect().await?);
                let service: Arc<dyn messaging::MessagingService> =
                    Arc::new(DefaultMessagingService::new(provider));
                Ok(Some(service))
            }
            #[cfg(not(feature = "greengrass"))]
            {
                Ok(None) // Greengrass IPC messaging requires the 'greengrass' feature.
            }
        }
    }
}

/// Common imports for component authors.
pub mod prelude {
    pub use crate::cli::{ConfigSourceSpec, ParsedArgs, RuntimeMode};
    pub use crate::config::model::Config;
    pub use crate::config::ConfigurationChangeListener;
    pub use crate::messaging::{
        message_handler, MessageHandler, MessagingService, Qos, ReplyFuture,
    };
    pub use crate::metrics::{Measure, Metric, MetricBuilder, MetricService};
    #[cfg(feature = "streaming")]
    pub use crate::streaming::{StreamHandle, StreamRecord, StreamService, Stats as StreamStats};
    pub use crate::{GgCommons, GgCommonsBuilder, GgError, Result};
}
