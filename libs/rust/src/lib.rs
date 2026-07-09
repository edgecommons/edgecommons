//! # EdgeCommons (Rust)
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
//! real-time device-shadow round-trip. See `../EDGECOMMONS_RUST_PORT.md` for the full
//! design and history.
//!
//! ```no_run
//! use edgecommons::prelude::*;
//!
//! # async fn run() -> edgecommons::Result<()> {
//! let gg = EdgeCommonsBuilder::new("com.example.MyComponent")
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
pub mod commands;
pub mod config;
#[cfg(feature = "credentials")]
pub mod credentials;
pub mod error;
pub mod facades;
pub mod health;
pub mod heartbeat;
mod instance;
#[cfg(feature = "greengrass")]
pub mod ipc;
pub mod logging;
pub mod logs;
pub mod messaging;
pub mod metrics;
#[cfg(feature = "parameters")]
pub mod parameters;
pub mod platform;
pub mod proto;
#[cfg(feature = "streaming")]
pub mod streaming;
pub mod uns;

#[cfg(test)]
mod testutil;

pub use error::{EdgeCommonsError, Result};
pub use instance::EdgeCommonsInstance;

use std::ffi::OsString;
use std::sync::Arc;

use arc_swap::ArcSwap;
use serde_json::Value;

use crate::cli::ParsedArgs;
use crate::config::model::Config;
use crate::platform::Transport;

/// The initialized component runtime. Holds the wired services and the current
/// configuration snapshot. Dropping it releases owned resources (RAII) — there is
/// no separate `close()` to forget.
pub struct EdgeCommons {
    component_name: String,
    args: ParsedArgs,
    config: Arc<ArcSwap<Config>>,
    messaging: Option<Arc<dyn messaging::MessagingService>>,
    logs: Arc<dyn logs::LogService>,
    metrics: Arc<dyn metrics::MetricService>,
    /// Telemetry streams (the `streaming` feature). Always present when the feature is on;
    /// empty if no `streaming` config section was provided.
    #[cfg(feature = "streaming")]
    streams: Arc<dyn streaming::StreamService>,
    /// Owns the streaming stats→metrics task; dropping `EdgeCommons` stops it (RAII).
    #[cfg(feature = "streaming")]
    _stream_metrics: Option<streaming::StreamMetricsBridge>,
    /// Credential service (the `credentials` feature). `None` when the component config has no
    /// `credentials` section.
    #[cfg(feature = "credentials")]
    credentials: Option<Arc<dyn credentials::CredentialService>>,
    /// Owns the credential stats→metrics task; dropping `EdgeCommons` stops it (RAII).
    #[cfg(feature = "credentials")]
    _credential_metrics: Option<credentials::CredentialMetricsBridge>,
    /// Parameter service (the `parameters` feature). `None` when the component config has no
    /// `parameters` section. Owns its background refresh thread; dropping `EdgeCommons` stops it.
    #[cfg(feature = "parameters")]
    parameters: Option<Arc<dyn parameters::ParameterService>>,
    /// Config-change listeners notified on hot reload.
    listeners: ConfigListeners,
    /// Shared readiness state (FR-HB-1/2): [`Self::set_ready`] toggles the ready flag, the SIGTERM
    /// watcher flips "shutting down", and the health server reads both (+ messaging connected) to
    /// answer `/readyz`.
    health_state: health::HealthState,
    /// Flipped to `true` by the signal watcher (FR-HB-2) on SIGTERM/Ctrl-C; awaited by
    /// [`Self::shutdown_signal`] so an app's run loop can exit on the library's termination signal
    /// instead of hand-rolling `tokio::signal`. Watch semantics mean a clone created after the
    /// signal still observes the latched `true` (no missed-notification race).
    shutdown_rx: tokio::sync::watch::Receiver<bool>,
    /// Owns the HTTP health server thread; dropping `EdgeCommons` stops it (RAII). `None` when health
    /// is disabled (the default off-KUBERNETES) or the listener failed to bind.
    _health_server: Option<health::HealthServer>,
    /// Owns the SIGTERM/Ctrl-C watcher task (FR-HB-2); aborted on drop.
    _signal_task: AbortOnDrop,
    /// Owns the `_bcast` republish listener (§9.3/§9.4, the late-join lever); dropping
    /// `EdgeCommons` unsubscribes it (RAII). Declared BEFORE `_heartbeat` so it tears down first
    /// (mirrors the Java canonical's shutdown order: `republishListener.close()` before
    /// `heartbeat.close()` — struct fields drop in declaration order). `None` when no messaging
    /// transport was wired.
    _republish_listener: Option<Arc<uns::RepublishListener>>,
    /// The library-owned command inbox (DESIGN-uns §9.5, slice S2, the minimal `commands()`
    /// facade); dropping `EdgeCommons` unsubscribes it (RAII). Declared BEFORE `_heartbeat` so it
    /// tears down before the heartbeat (mirrors the Java canonical's shutdown order:
    /// `commandInbox.close()` before `heartbeat.close()`), and AFTER `_republish_listener`
    /// (mirrors `republishListener.close()` before `commandInbox.close()`). `None` when no
    /// messaging transport was wired.
    commands: Option<Arc<commands::CommandInbox>>,
    /// Owns the heartbeat task; dropping `EdgeCommons` stops it (RAII). `Arc`-shared with the
    /// republish listener's `republish-state` action ([`heartbeat::Heartbeat::publish_state_now`])
    /// and the command inbox's `ping` uptime source ([`heartbeat::Heartbeat::uptime_secs`]).
    _heartbeat: Arc<heartbeat::Heartbeat>,
    /// The `data()` facade's stream-route seam (DESIGN-class-facades §4): `Some` when the
    /// `streaming` cargo feature is on (regardless of whether a `streaming` config section is
    /// present — the underlying `StreamService` is always available under the feature, and an
    /// unconfigured stream NAME fails at append time, logged and swallowed like any other
    /// stream-route failure); `None` when the feature is off, so a `stream:` channel on
    /// `data()` falls back to a LOCAL publish (D1a).
    facade_stream_sink: Option<Arc<dyn facades::StreamSink>>,
    /// The injected "now" seam the `data()`/`events()` facades use for their `serverTs`/
    /// `timestamp` defaults (DESIGN-class-facades). Production is always
    /// [`facades::system_clock`] — the facades' own unit/conformance tests inject a fixed clock
    /// directly (they build `EdgeCommonsInstance`/the facades without going through this runtime).
    facade_clock: facades::Clock,
    /// Owns the hot-reload task; aborted on drop. `None` if the source can't watch.
    _reload_task: Option<AbortOnDrop>,
    /// Keeps the config source (and its OS file watcher) alive for hot reload; also cloned into
    /// the command inbox's `reload-config` action ([`reload_from_provider`]).
    _config_source: Arc<dyn config::source::ConfigSource>,
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

impl EdgeCommons {
    /// The component's full name.
    pub fn component_name(&self) -> &str {
        &self.component_name
    }

    /// The parsed standard CLI arguments.
    pub fn args(&self) -> &ParsedArgs {
        &self.args
    }

    /// Resolve when the library observes a termination signal — SIGTERM on Unix (what Greengrass /
    /// the kubelet send to stop), or Ctrl-C on any platform — the same signal that flips `/readyz`
    /// to 503 (FR-HB-2). Await this from your run loop instead of re-implementing `tokio::signal`,
    /// so the library remains the single signal source. Returns immediately if a termination signal
    /// has already been observed.
    ///
    /// ```no_run
    /// # async fn run(gg: &edgecommons::EdgeCommons) {
    /// gg.shutdown_signal().await; // resolves on SIGTERM / Ctrl-C
    /// # }
    /// ```
    pub async fn shutdown_signal(&self) {
        let mut rx = self.shutdown_rx.clone();
        wait_for_shutdown(&mut rx).await;
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
    /// | `EdgeCommonsError::Messaging` | No messaging service was wired | Select a transport (`--transport IPC|MQTT`) and enable the matching cargo feature |
    ///
    /// Note: since Phase 0, an unsupported transport/feature combination fails fast during
    /// `build()` (e.g. `--platform GREENGRASS` without the `greengrass` feature), so a wired
    /// runtime normally always has a messaging service.
    pub fn messaging(&self) -> Result<Arc<dyn messaging::MessagingService>> {
        self.messaging.clone().ok_or_else(|| {
            EdgeCommonsError::Messaging(
                "messaging is not available: select --transport IPC (requires the 'greengrass' \
                 feature) or --transport MQTT (requires the 'standalone' feature)"
                    .to_string(),
            )
        })
    }

    /// The log bus publisher for this component.
    pub fn logs(&self) -> Arc<dyn logs::LogService> {
        self.logs.clone()
    }

    /// The metric service for this component (the testable seam).
    pub fn metrics(&self) -> Arc<dyn metrics::MetricService> {
        self.metrics.clone()
    }

    /// The command-inbox facade (DESIGN-uns §9.5, slice S2 — the minimal `commands()` facade):
    /// register custom command verbs with `gg.commands().register(verb, handler)`. The built-in
    /// verbs (`ping`, `reload-config`, `get-configuration`) are registered by the library and
    /// cannot be shadowed. `None` only when no messaging transport was wired (which the builder
    /// never leaves unset in practice — every supported transport either wires messaging or fails
    /// `build()` outright), mirroring `EdgeCommons.getCommands()`, which is `null` only on a
    /// mock/subclass bring-up that never ran `init`.
    pub fn commands(&self) -> Option<Arc<commands::CommandInbox>> {
        self.commands.clone()
    }

    /// The UNS topic builder + validator bound to this component's resolved
    /// identity (instance `"main"`) and its `topic.includeRoot` setting
    /// (UNS-CANONICAL-DESIGN §2 — [`uns::Uns`] applies the root per-target only for
    /// multi-level hierarchies, D-U25). Built over the CURRENT config snapshot; for
    /// instance-scoped topics use [`Self::instance`]`?.uns()`.
    pub fn uns(&self) -> uns::Uns {
        let cfg = self.config.load_full();
        uns::Uns::new(cfg.identity().clone(), cfg.topic_include_root())
    }

    /// The instance-scoped handle for an instance token (UNS-CANONICAL-DESIGN §3,
    /// D-U3): a [`EdgeCommonsInstance`] whose `uns()` mints topics with — and whose
    /// `message(...)` stamps envelopes with — this instance token. The token is
    /// validated against the §2.2 token rule; the id is deliberately NOT verified
    /// against the configured `component.instances[]` (instances may be created
    /// dynamically) — an unknown id is only logged at DEBUG as a diagnostic aid.
    ///
    /// # Errors
    /// [`EdgeCommonsError::UnsValidation`] when the token violates the §2.2 token rule.
    pub fn instance(&self, instance_id: &str) -> Result<EdgeCommonsInstance> {
        uns::check_token(instance_id, "instance id")?;
        let cfg = self.config.load_full();
        let configured = cfg.instance_ids();
        if !configured.iter().any(|id| id == instance_id) {
            tracing::debug!(
                instance = %instance_id,
                configured = ?configured,
                "instance id is not among the configured component.instances[] ids - \
                 creating a dynamic instance handle"
            );
        }
        EdgeCommonsInstance::new(
            instance_id.to_string(),
            cfg,
            self.messaging.clone(),
            self.facade_stream_sink.clone(),
            self.facade_clock.clone(),
        )
    }

    /// The `data()` publish facade for the component's `main` instance — the
    /// single-instance-component convenience, equivalent to `instance("main").data()`
    /// (DESIGN-class-facades §3, D6). Builds/validates the `SouthboundSignalUpdate` body.
    pub fn data(&self) -> facades::DataFacade {
        self.instance(messaging::MessageIdentity::DEFAULT_INSTANCE)
            .expect("the 'main' instance token always passes the §2.2 token rule")
            .data()
    }

    /// The `events()` publish facade for the component's `main` instance — equivalent to
    /// `instance("main").events()` (DESIGN-class-facades §3, D6). Operator events & alarms on
    /// the `evt` class.
    pub fn events(&self) -> facades::EventsFacade {
        self.instance(messaging::MessageIdentity::DEFAULT_INSTANCE)
            .expect("the 'main' instance token always passes the §2.2 token rule")
            .events()
    }

    /// The `app()` publish facade for the component's `main` instance — equivalent to
    /// `instance("main").app()` (DESIGN-class-facades §3, D6). Free-form inter-component
    /// pub/sub on the `app` class.
    pub fn app(&self) -> facades::AppFacade {
        self.instance(messaging::MessageIdentity::DEFAULT_INSTANCE)
            .expect("the 'main' instance token always passes the §2.2 token rule")
            .app()
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

    /// The parameter service for this component (the `parameters` feature), or `None` when the
    /// config has no `parameters` section. Offline-first reads of externalized config from the
    /// configured source (SSM, mounted dir, env, …). Sibling to [`Self::credentials`].
    #[cfg(feature = "parameters")]
    pub fn parameters(&self) -> Option<Arc<dyn parameters::ParameterService>> {
        self.parameters.clone()
    }

    /// Set the component's readiness flag (FR-HB-1 readiness model).
    ///
    /// # Purpose
    /// Gate the health `/readyz` (and `/startupz`) endpoint on the app's own startup work. The flag
    /// **defaults to `true`**, so a component is reported ready as soon as messaging connects. An
    /// app that must finish setup first (e.g. confirm required subscriptions) calls
    /// `set_ready(false)` early and `set_ready(true)` once ready.
    ///
    /// # Semantics
    /// `/readyz` returns 200 only when `messaging-connected && ready && !shutting_down`. This flag
    /// is the `ready` term; it has no effect on `/livez` (liveness never depends on app state).
    /// Idempotent and thread-safe; a no-op if the health server is disabled.
    pub fn set_ready(&self, ready: bool) {
        self.health_state.set_ready(ready);
    }

    /// Registers the component's per-instance connectivity provider — the overridable surface for
    /// reporting connectivity AT THE INSTANCE LEVEL (each configured connection's health) in the
    /// `main` `state` keepalive's `instances[]`, without minting a separate UNS instance per
    /// connection (data + lifecycle stay under `main`; the #1c model). A reference adapter maps each
    /// connection to its reachability: OPC UA server session / Modbus slave / file-replicator source
    /// directory. Pass `None` to stop reporting.
    pub fn set_instance_connectivity_provider(
        &self,
        provider: Option<Arc<heartbeat::InstanceConnectivityProvider>>,
    ) {
        self._heartbeat.set_instance_connectivity_provider(provider);
    }

    /// Whether the runtime has begun shutting down (the SIGTERM watcher fired). Exposed so an app
    /// run loop can cooperatively exit; `/readyz` already reports 503 once this is true.
    pub fn is_shutting_down(&self) -> bool {
        self.health_state.is_shutting_down()
    }

    /// Register a listener invoked after the configuration is hot-reloaded.
    ///
    /// Mirrors the Java/Python `addConfigurationChangeListener`. The listener fires on
    /// successful reloads of a watchable config source (e.g. `FILE`).
    pub fn add_config_change_listener(
        &self,
        listener: Arc<dyn config::ConfigurationChangeListener>,
    ) {
        if let Ok(mut listeners) = self.listeners.lock() {
            listeners.push(listener);
        }
    }

    /// Remove a previously-registered config-change listener (by identity).
    pub fn remove_config_change_listener(
        &self,
        listener: &Arc<dyn config::ConfigurationChangeListener>,
    ) {
        if let Ok(mut listeners) = self.listeners.lock() {
            listeners.retain(|existing| !Arc::ptr_eq(existing, listener));
        }
    }
}

/// Fluent builder for [`EdgeCommons`] (the supported construction path).
pub struct EdgeCommonsBuilder {
    component_name: String,
    argv: Option<Vec<OsString>>,
    receive_own_messages: bool,
}

impl EdgeCommonsBuilder {
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
    pub async fn build(self) -> Result<EdgeCommons> {
        let parsed = match self.argv {
            Some(argv) => cli::parse_from(argv)?,
            None => cli::parse_from(std::env::args_os())?,
        };

        // Identity (thing name) was resolved by the platform resolver during arg parse
        // (explicit -t ▸ [KUBERNETES: EDGECOMMONS_THING_NAME ▸ POD_NAME] ▸ AWS_IOT_THING_NAME env
        // probe ▸ library fallback, DESIGN-core §6.2 / FR-RT-7).
        let thing_name = parsed.identity.clone();

        // Messaging is initialized first: it depends only on the resolved transport (the
        // MQTT messaging config / IPC socket), not on the component config — and the
        // CONFIG_COMPONENT source needs a messaging handle to fetch the config. The
        // transport-injection site (DESIGN-core §4.2) branches on the resolved Transport,
        // not a legacy mode enum. The CONCRETE service handle is kept so the UNS knobs
        // (guard includeRoot, request-deadline default) can be late-bound after the
        // config loads, and so the crate-private reserved-publish seam (§4.2) can be
        // handed to the library's own publishers.
        let messaging_impl = init_messaging(
            parsed.transport,
            parsed.messaging_config_path.as_deref(),
            self.receive_own_messages,
        )
        .await?;
        let messaging: Option<Arc<dyn messaging::MessagingService>> = messaging_impl
            .clone()
            .map(|s| s as Arc<dyn messaging::MessagingService>);
        let reserved: Option<Arc<dyn messaging::ReservedMessaging>> = messaging_impl
            .clone()
            .map(|s| s as Arc<dyn messaging::ReservedMessaging>);

        // `Arc`, not `Box`: the `reload-config` command action (below) needs a long-lived clone
        // alongside the final `_config_source` field.
        let source: Arc<dyn config::source::ConfigSource> = Arc::from(config::source::build(
            &parsed.config,
            messaging.clone(),
            &thing_name,
            &self.component_name,
        )?);
        let source: Arc<dyn config::source::ConfigSource> =
            Arc::new(config::layered::LayeredConfigSource::new(
                source,
                parsed.config.clone(),
                &self.component_name,
            ));
        let raw = source.load().await?;
        config::validation::validate(&raw)?;
        let cfg = Config::from_value(self.component_name.clone(), thing_name.clone(), raw)?;

        // UNS late-binds (§1.5 init order — messaging exists BEFORE config): the
        // request() deadline default (messaging.requestTimeoutSeconds, §5/D-U5;
        // until now the built-in 30 s applied, deliberately, so the CONFIG_COMPONENT
        // bootstrap request had a deadline) and the reserved-class guard's
        // includeRoot flag — bound to the EFFECTIVE root (includeRoot AND a
        // multi-level hierarchy, D-U27) so the guard's position-5 check agrees with
        // topic building, which no-ops includeRoot on a single-level hierarchy (D-U25).
        if let Some(service) = &messaging_impl {
            service.set_default_request_timeout(cfg.messaging_request_timeout());
            service.set_guard_include_root(cfg.effective_include_root());
        }
        let config: Arc<ArcSwap<Config>> = Arc::new(ArcSwap::from_pointee(cfg));
        let logs = logs::DefaultLogService::start(config.clone(), reserved.clone())?;

        // Logging is configured from the component CONFIG, which loads after the resolver. The
        // resolved platform is already known, so its profile's default logging format (json on
        // KUBERNETES — FR-LOG-1) is threaded in to seed the format when the config omits one
        // (precedence FR-RT-3: explicit config ▸ profile default ▸ library default).
        let profile_logging_default =
            crate::platform::profile(parsed.platform).and_then(|p| p.logging_format);
        logging::init(&config.load_full(), profile_logging_default);

        // Deferred early-bootstrap observability: the platform-resolver summary and the messaging
        // connection happen BEFORE the tracing subscriber is installed (above), so they are emitted
        // here, immediately after `logging::init`, where they can actually be captured. The config
        // source is rendered as its short CLI token (the same tokens accepted by `-c`).
        let config_source_token = match parsed.config {
            crate::cli::ConfigSourceSpec::File { .. } => "FILE",
            crate::cli::ConfigSourceSpec::ConfigMap { .. } => "CONFIGMAP",
            crate::cli::ConfigSourceSpec::Env { .. } => "ENV",
            crate::cli::ConfigSourceSpec::Greengrass { .. } => "GG_CONFIG",
            crate::cli::ConfigSourceSpec::Shadow { .. } => "SHADOW",
            crate::cli::ConfigSourceSpec::ConfigComponent => "CONFIG_COMPONENT",
        };
        tracing::info!(
            "platform resolved: platform={:?} transport={:?} configSource={} identity={}",
            parsed.platform,
            parsed.transport,
            config_source_token,
            parsed.identity
        );
        if messaging.as_ref().is_some_and(|m| m.connected()) {
            tracing::info!("messaging connected (transport={:?})", parsed.transport);
        }

        let thing_for_log = config.load_full().thing_name.clone();
        tracing::info!(
            component = %self.component_name,
            thing = %thing_for_log,
            config_source = source.source_name(),
            "EdgeCommons initialized"
        );

        let snapshot = config.load_full();
        // The resolved platform threads the metric-target profile default into target selection the
        // same way logging-format/health-enabled are threaded (FR-MET-1 / FR-RT-3): the effective
        // target is `explicit metricEmission.target ▸ profile default (prometheus on KUBERNETES) ▸
        // log`. No resolver→ConfigManager dependency is added — the platform is already known here.
        let emitter = Arc::new(
            metrics::MetricEmitter::new_internal(
                &snapshot,
                messaging.clone(),
                reserved.clone(),
                parsed.platform,
            )
            .await?,
        );
        let metrics: Arc<dyn metrics::MetricService> = emitter.clone();
        // §4.3: the heartbeat is the UNS state keepalive + sys metric; the state
        // class is reserved, so it publishes through the crate-private seam. `Arc`-wrapped so
        // the _bcast republish listener's `republish-state` action can share it (§9.3/§9.4,
        // below).
        let heartbeat = Arc::new(heartbeat::Heartbeat::start(
            config.clone(),
            metrics.clone(),
            reserved.clone(),
        ));

        // Credentials / local vault (feature-gated): open the shared vault when the config has a
        // `credentials` section, resolving path templates ({ThingName}/{ComponentFullName}) first.
        // Opened before streaming so the streaming config can reference vault secrets. `None` when
        // no section is present.
        #[cfg(feature = "credentials")]
        let (credentials, credential_metrics) = {
            let creds: Option<Arc<dyn credentials::CredentialService>> = match snapshot
                .raw
                .get("credentials")
            {
                None => None,
                Some(value) => {
                    let mut cfg: credentials::CredentialsConfig =
                        serde_json::from_value(value.clone())?;
                    cfg.vault.path = config::template::resolve(&snapshot, &cfg.vault.path);
                    if let Some(kp) = cfg.vault.key_provider.key_path.as_mut() {
                        *kp = config::template::resolve(&snapshot, kp);
                    }
                    // Transparently namespace every key by <thingName>/<componentName> so a shared
                    // device vault / fleet central store can't collide across components or devices.
                    let namespace = format!("{}/{}", snapshot.thing_name, self.component_name);
                    // Platform-default KEK custodian (FR-CRED-6 / FR-RT-3): when `keyProvider.type`
                    // is unspecified, fall back to the resolved platform's profile default (env on
                    // KUBERNETES) before the library default `file`. Threaded the same way the
                    // logging-format / metric-target defaults are; an explicit type always wins.
                    // This only changes the default provider TYPE — it never enables credentials
                    // (we are already inside the `Some(credentials section present)` arm).
                    let default_kind =
                        crate::platform::profile_credentials_key_provider(parsed.platform);
                    let svc =
                        credentials::open_namespaced_with_default(&cfg, &namespace, default_kind)?;
                    Some(Arc::new(svc) as Arc<dyn credentials::CredentialService>)
                }
            };
            // Bridge non-sensitive credential stats into the metric targets (RAII; aborts on drop).
            let bridge = creds
                .as_ref()
                .map(|c| credentials::CredentialMetricsBridge::start(c.clone(), metrics.clone()));
            (creds, bridge)
        };

        // Parameters (feature-gated): open the parameter service when the config has a `parameters`
        // section. Sibling to credentials — externalized config from a pluggable source, offline-first.
        // The persistent-cache path is templated ({ThingName}/{ComponentFullName}) like the vault.
        #[cfg(feature = "parameters")]
        let params: Option<Arc<dyn parameters::ParameterService>> =
            match snapshot.raw.get("parameters") {
                None => None,
                Some(value) => {
                    let mut cfg: parameters::ParametersConfig =
                        serde_json::from_value(value.clone())?;
                    cfg.cache.path = config::template::resolve(&snapshot, &cfg.cache.path);
                    if let Some(kp) = cfg.cache.key_provider.key_path.as_mut() {
                        *kp = config::template::resolve(&snapshot, kp);
                    }
                    let svc = parameters::open(&cfg)?;
                    Some(Arc::new(svc) as Arc<dyn parameters::ParameterService>)
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

        // The `data()` facade's stream-route seam (DESIGN-class-facades §4): composes the
        // `streaming` feature's `StreamService` when it is compiled in (the underlying service is
        // always available under the feature, even with zero configured streams — an unconfigured
        // stream NAME then fails at append time, caught and logged like any other stream-route
        // failure); `None` when the feature is off, so a `data().via(Channel::stream(name))` call
        // falls back to a LOCAL publish instead (readiness / no-streaming → local, D1a).
        #[cfg(feature = "streaming")]
        let facade_stream_sink: Option<Arc<dyn facades::StreamSink>> =
            Some(Arc::new(streaming::StreamServiceSink::new(streams.clone())));
        #[cfg(not(feature = "streaming"))]
        let facade_stream_sink: Option<Arc<dyn facades::StreamSink>> = None;
        // The injected "now" seam for the `data()`/`events()` facades' `serverTs`/`timestamp`
        // defaults (no inline `Instant`/`SystemTime` read in a facade body).
        let facade_clock: facades::Clock = facades::system_clock();

        // Health / readiness (FR-HB-1/2). The shared readiness state seeds both the HTTP health
        // endpoint and the SIGTERM watcher. `ready` defaults to true and messaging-connected is
        // queried live, so a component is ready as soon as the broker connects unless the app gates
        // it via `set_ready(false)`.
        let health_state = health::HealthState::new(messaging.clone());

        // The health server is enabled by: explicit config `health.enabled` ▸ the platform-profile
        // default (on for KUBERNETES) ▸ off (precedence FR-RT-3). The resolved platform is known
        // here, reusing the same threading as the logging default — no resolver→ConfigManager dep.
        let health_enabled = health::resolve_enabled(&snapshot.parsed.health, parsed.platform);
        let health_server = if health_enabled {
            let hc = &snapshot.parsed.health;
            let server_cfg = health::ServerConfig {
                port: hc.port(),
                liveness_path: hc.liveness_path().to_string(),
                readiness_path: hc.readiness_path().to_string(),
                startup_path: hc.startup_path().to_string(),
            };
            match health::HealthServer::start(server_cfg, health_state.clone()) {
                Ok(server) => Some(server),
                Err(e) => {
                    // A bind failure must not take down the component (health is auxiliary).
                    tracing::error!(error = %e, port = hc.port(), "failed to start health server");
                    None
                }
            }
        } else {
            None
        };

        // FR-HB-2: the LIBRARY wires SIGTERM (Unix) / Ctrl-C so a kubelet stop flips `/readyz` to
        // 503 at once. The library does not own the run loop, so it cannot exit the process —
        // resource teardown stays RAII on `EdgeCommons` drop when the app leaves its loop. The watcher
        // only flips the (idempotent) shutting-down flag and logs.
        // Watch channel the signal watcher flips on shutdown; `EdgeCommons::shutdown_signal` awaits it
        // so apps await one library-owned future instead of hand-rolling `tokio::signal`.
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let signal_task = AbortOnDrop(spawn_signal_watcher(
            health_state.clone(),
            Arc::new(shutdown_tx),
        ));

        // Internal listeners reconfigure the metric target and logging on hot reload.
        let listeners: ConfigListeners = Arc::new(std::sync::Mutex::new(Vec::new()));
        if let Ok(mut l) = listeners.lock() {
            l.push(emitter as Arc<dyn config::ConfigurationChangeListener>);
            l.push(logs.clone() as Arc<dyn config::ConfigurationChangeListener>);
            l.push(Arc::new(logging::LoggingReconfigurer)
                as Arc<dyn config::ConfigurationChangeListener>);
        }

        // §4.3: announce the effective (redacted) configuration on the UNS cfg topic
        // — the startup push; registered as a listener so every hot reload
        // re-announces. Best-effort (publish_now never fails the build).
        let mut republish_listener: Option<Arc<uns::RepublishListener>> = None;
        if let Some(reserved) = &reserved {
            let cfg_publisher = Arc::new(config::effective::EffectiveConfigPublisher::new(
                reserved.clone(),
            ));
            cfg_publisher.publish_now(&snapshot).await;

            // §9.3/§9.4: the _bcast republish listener (the late-join lever) — subscribe the
            // own-device broadcast topics on the PRIMARY connection so the uns-bridge's
            // reconnect-rehydration broadcast (and a console's explicit republish) gets a
            // jittered, coalesced state/cfg re-announce. Always on (no config surface);
            // best-effort start (a failure disables the listener only). Requires a messaging
            // service to subscribe on, which is always Some here (reserved and messaging are
            // both derived from the same messaging_impl).
            if let Some(messaging_svc) = &messaging {
                let hb_for_republish = heartbeat.clone();
                let state_action: uns::RepublishAction = Arc::new(move || {
                    let hb = hb_for_republish.clone();
                    Box::pin(async move { hb.publish_state_now().await })
                });
                let cfg_publisher_for_republish = cfg_publisher.clone();
                let config_for_republish = config.clone();
                let cfg_action: uns::RepublishAction = Arc::new(move || {
                    let publisher = cfg_publisher_for_republish.clone();
                    let cfg = config_for_republish.clone();
                    Box::pin(async move { publisher.publish_now(&cfg.load_full()).await })
                });
                let listener =
                    uns::RepublishListener::new(messaging_svc.clone(), state_action, cfg_action);
                let device = snapshot.identity().device().to_string();
                listener.clone().start(&device).await;
                republish_listener = Some(listener);
            }

            if let Ok(mut l) = listeners.lock() {
                l.push(cfg_publisher as Arc<dyn config::ConfigurationChangeListener>);
            }
        }

        // §9.5 (slice S2): the component's own command inbox
        // (ecv1/{device}/{component}/main/cmd/#) — built-ins ping / reload-config /
        // get-configuration answer the console out of the box; apps add custom verbs via
        // `EdgeCommons::commands()`. Always on (no config surface); best-effort start (a failure
        // disables the inbox only). Wired right after the republish listener (needs only the
        // messaging service, not the privileged reserved-publish seam).
        let commands = if let Some(messaging_svc) = &messaging {
            let uptime_secs: Arc<dyn Fn() -> u64 + Send + Sync> = {
                let hb = heartbeat.clone();
                Arc::new(move || hb.uptime_secs())
            };
            let reload_action: commands::ReloadAction = {
                let source = source.clone();
                let config = config.clone();
                let listeners = listeners.clone();
                let component_name = self.component_name.clone();
                let thing_name = thing_name.clone();
                Arc::new(move || {
                    let source = source.clone();
                    let config = config.clone();
                    let listeners = listeners.clone();
                    let component_name = component_name.clone();
                    let thing_name = thing_name.clone();
                    Box::pin(async move {
                        reload_from_provider(
                            source.as_ref(),
                            &config,
                            &listeners,
                            &component_name,
                            &thing_name,
                        )
                        .await
                    })
                })
            };
            let redacted_config: Arc<dyn Fn() -> Option<Value> + Send + Sync> = {
                let config = config.clone();
                Arc::new(move || Some(config::effective::redact(&config.load_full().raw)))
            };
            let inbox = commands::CommandInbox::new(
                messaging_svc.clone(),
                config.clone(),
                uptime_secs,
                reload_action,
                redacted_config,
            );
            inbox.clone().start().await;
            Some(inbox)
        } else {
            None
        };

        let reload_task = source.watch().map(|updates| {
            spawn_config_reload(
                updates,
                config.clone(),
                listeners.clone(),
                self.component_name.clone(),
                thing_name,
            )
        });

        Ok(EdgeCommons {
            component_name: self.component_name,
            args: parsed,
            config,
            messaging,
            logs,
            metrics,
            #[cfg(feature = "streaming")]
            streams,
            #[cfg(feature = "streaming")]
            _stream_metrics: stream_metrics,
            #[cfg(feature = "credentials")]
            credentials,
            #[cfg(feature = "credentials")]
            _credential_metrics: credential_metrics,
            #[cfg(feature = "parameters")]
            parameters: params,
            listeners,
            health_state,
            shutdown_rx,
            _health_server: health_server,
            _signal_task: signal_task,
            _republish_listener: republish_listener,
            commands,
            facade_stream_sink,
            facade_clock,
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
            apply_reloaded_config(raw, &config, &listeners, &component_name, &thing_name).await;
        }
    }))
}

/// Validates and applies a raw config document: validate against the schema, parse, atomically
/// swap the live snapshot, and notify listeners. Returns `true` on success (the previous
/// configuration is kept on any failure — a reload/push must never crash the component).
///
/// Shared by the watch-driven hot-reload loop ([`spawn_config_reload`]) and the pull-based
/// `reload-config` command action ([`reload_from_provider`]), so BOTH paths funnel through the
/// exact same apply site: `config` (the `ArcSwap` read by [`EdgeCommons::config`],
/// [`config::effective::redact`], and every subsystem) is always the freshly applied snapshot the
/// instant either path returns `true` — there is no separate "full config" copy that could go
/// stale, unlike a design that caches the applied document in a second field.
async fn apply_reloaded_config(
    raw: serde_json::Value,
    config: &Arc<ArcSwap<Config>>,
    listeners: &ConfigListeners,
    component_name: &str,
    thing_name: &str,
) -> bool {
    if let Err(e) = config::validation::validate(&raw) {
        tracing::warn!(error = %e, "reloaded config failed validation; keeping previous");
        return false;
    }
    match Config::from_value(component_name.to_string(), thing_name.to_string(), raw) {
        Ok(new_config) => {
            let snapshot = Arc::new(new_config);
            config.store(snapshot.clone());
            tracing::info!("configuration reloaded");
            let current = listeners.lock().map(|l| l.clone()).unwrap_or_default();
            for listener in current {
                let _ = listener.on_configuration_change(snapshot.clone()).await;
            }
            true
        }
        Err(e) => {
            tracing::warn!(error = %e, "reloaded config could not be parsed; keeping previous");
            false
        }
    }
}

/// Re-fetches the configuration from the active config source and re-applies it — the
/// `reload-config` command verb's action (DESIGN-uns §9.5, [`commands::CommandInbox`]).
/// Re-invokes the source's [`config::source::ConfigSource::load`] (re-reads the file/ConfigMap/
/// env, or re-requests from `CONFIG_COMPONENT`), then delegates validation + apply +
/// listener-notification to [`apply_reloaded_config`] — the SAME apply path a watched hot-reload
/// uses, so the `cfg` publisher and `get-configuration` always observe the freshly applied
/// snapshot afterward. Best-effort: any failure is logged and `false` returned — a reload never
/// crashes a running component.
async fn reload_from_provider(
    source: &dyn config::source::ConfigSource,
    config: &Arc<ArcSwap<Config>>,
    listeners: &ConfigListeners,
    component_name: &str,
    thing_name: &str,
) -> bool {
    match source.load().await {
        Ok(raw) => apply_reloaded_config(raw, config, listeners, component_name, thing_name).await,
        Err(e) => {
            tracing::warn!(
                error = %e,
                source = source.source_name(),
                "reload-config: re-fetch from the active config source failed"
            );
            false
        }
    }
}

/// Spawn the SIGTERM/Ctrl-C watcher (FR-HB-2).
///
/// # Purpose
/// On the first termination signal it flips the readiness state to "shutting down" so the health
/// `/readyz` endpoint returns 503 immediately (the kubelet stops routing traffic before the pod
/// goes away), then logs and ends. The library cannot exit the process (it does not own the run
/// loop); resource teardown remains RAII on [`EdgeCommons`] drop when the app leaves its loop.
///
/// # Semantics
/// Idempotent — [`health::HealthState::begin_shutdown`] is a flag store, safe under repeated
/// signals. The returned [`tokio::task::JoinHandle`] is held in an [`AbortOnDrop`] so the watcher
/// is cleaned up if the runtime is dropped before any signal arrives.
fn spawn_signal_watcher(
    state: health::HealthState,
    shutdown_tx: Arc<tokio::sync::watch::Sender<bool>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        wait_for_terminate().await;
        state.begin_shutdown();
        // Latch the shutdown flag so `EdgeCommons::shutdown_signal` resolves (and stays resolved for
        // any later-cloned receiver). Ignore the error when there are no receivers.
        let _ = shutdown_tx.send(true);
        tracing::info!("termination signal received; readiness set to 503 (shutting down)");
    })
}

/// Resolve once `rx` observes a shutdown (value `true`), returning immediately if it already has.
/// Backs [`EdgeCommons::shutdown_signal`]; an `Err` (all senders dropped) is treated as shutdown.
async fn wait_for_shutdown(rx: &mut tokio::sync::watch::Receiver<bool>) {
    let _ = rx.wait_for(|flag| *flag).await;
}

/// Resolve on SIGTERM (Unix — the signal Greengrass / the kubelet send to stop) or Ctrl-C (all
/// platforms). On Unix, falls back to Ctrl-C if the SIGTERM handler cannot be installed.
async fn wait_for_terminate() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
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

/// Initialize the messaging service for the resolved [`Transport`] (DESIGN-core §4.2 — the
/// transport-injection site).
///
/// # Purpose
/// For [`Transport::Mqtt`], load the messaging config and connect the dual-broker MQTT
/// provider; for [`Transport::Ipc`], connect the Greengrass IPC provider.
///
/// # Semantics
/// - The IPC lock (DESIGN-core §4.1) is enforced earlier by the resolver, so `Transport::Ipc`
///   reaching here implies `platform == GREENGRASS`.
/// - **Compile-time capability (Rust-specific fail-fast, DECISION §12 #4):** if
///   `Transport::Ipc` is selected but the binary was built without the `greengrass` cargo
///   feature, this **fails fast** naming the missing feature — replacing the historical
///   silent `Ok(None)` no-op.
/// - `receive_own_messages` is honored only on the IPC transport (matching the Java
///   `receiveOwnMessages` contract, which "applies only when messaging target is IPC").
///
/// # Errors
/// | Error Variant | Condition | Recovery |
/// |---------------|-----------|----------|
/// | `EdgeCommonsError::Io` / `EdgeCommonsError::Json` | Messaging config file missing or malformed | Check the `--transport MQTT <path>` file |
/// | `EdgeCommonsError::Messaging` | Broker/IPC connection failed; MQTT path missing; or the required cargo feature is disabled | Verify the broker/Nucleus; supply the path; enable the feature |
async fn init_messaging(
    transport: Transport,
    messaging_config_path: Option<&std::path::Path>,
    receive_own_messages: bool,
) -> Result<Option<Arc<messaging::DefaultMessagingService>>> {
    match transport {
        Transport::Mqtt => {
            #[cfg(feature = "standalone")]
            {
                use crate::messaging::config::MessagingConfig;
                use crate::messaging::provider::mqtt::MqttProvider;
                use crate::messaging::service::DefaultMessagingService;

                let path = messaging_config_path.ok_or_else(|| {
                    EdgeCommonsError::Cli(
                        "MQTT transport requires a messaging config path: \
                         --transport MQTT <messaging_config.json>"
                            .to_string(),
                    )
                })?;
                let mc = MessagingConfig::load(path).await?;
                let provider = Arc::new(MqttProvider::connect(&mc).await?);
                let qos = mc.messaging.qos_config();
                Ok(Some(Arc::new(DefaultMessagingService::new_with_qos(
                    provider, &qos,
                ))))
            }
            #[cfg(not(feature = "standalone"))]
            {
                let _ = messaging_config_path;
                Err(EdgeCommonsError::Messaging(
                    "MQTT transport requires the 'standalone' cargo feature".to_string(),
                ))
            }
        }
        Transport::Ipc => {
            #[cfg(feature = "greengrass")]
            {
                use crate::messaging::provider::ipc::IpcProvider;
                use crate::messaging::service::DefaultMessagingService;

                // The Greengrass Rust SDK exposes no IPC ReceiveMode, so own-message
                // suppression cannot be performed natively. `receiveOwnMessages=false` is a
                // documented no-op on IPC; warn so the developer is not surprised. (Honoring
                // the flag awaits an upstream SDK ReceiveMode addition.)
                if !receive_own_messages {
                    tracing::warn!(
                        "receiveOwnMessages=false is not supported by the Greengrass Rust SDK \
                         (no IPC ReceiveMode); proceeding as if true — the component WILL \
                         receive its own messages on subscribed topics"
                    );
                }
                let provider = Arc::new(IpcProvider::connect().await?);
                Ok(Some(Arc::new(DefaultMessagingService::new(provider))))
            }
            #[cfg(not(feature = "greengrass"))]
            {
                let _ = receive_own_messages;
                // Fail fast (DECISION §12 #4): GREENGRASS/IPC was selected (explicitly or by
                // auto-detection) but this binary lacks the `greengrass` cargo feature.
                // Replaces the historical silent `Ok(None)`.
                Err(EdgeCommonsError::Messaging(
                    "IPC transport (platform=GREENGRASS) requires the 'greengrass' cargo \
                     feature, which is absent from this build. Rebuild with \
                     --features greengrass (Linux/WSL only), or run with \
                     --platform HOST --transport MQTT <messaging_config.json>."
                        .to_string(),
                ))
            }
        }
    }
}

/// Common imports for component authors.
pub mod prelude {
    pub use crate::cli::{ConfigSourceSpec, ParsedArgs};
    pub use crate::commands::{CommandError, CommandHandler, CommandInbox, command_handler};
    pub use crate::config::ConfigurationChangeListener;
    pub use crate::config::model::Config;
    pub use crate::facades::{
        AppFacade, Channel, DataFacade, EventsFacade, Quality, Sample, Severity, SignalUpdate,
    };
    pub use crate::heartbeat::{InstanceConnectivity, InstanceConnectivityProvider};
    pub use crate::logs::{LogLevel, LogRecord, LogService, LogStats};
    pub use crate::messaging::{
        MessageHandler, MessageIdentity, MessagingService, Qos, ReplyFuture, message_handler,
    };
    pub use crate::metrics::{Measure, Metric, MetricBuilder, MetricService};
    pub use crate::platform::{Platform, Transport};
    #[cfg(feature = "streaming")]
    pub use crate::streaming::{Stats as StreamStats, StreamHandle, StreamRecord, StreamService};
    pub use crate::uns::{Uns, UnsClass, UnsScope};
    pub use crate::{
        EdgeCommons, EdgeCommonsBuilder, EdgeCommonsError, EdgeCommonsInstance, Result,
    };
}

#[cfg(test)]
mod shutdown_tests {
    //! Tests for the library-owned shutdown future (#17) backing `EdgeCommons::shutdown_signal`.
    use super::wait_for_shutdown;
    use std::time::Duration;

    #[tokio::test]
    async fn resolves_when_watcher_flips_the_flag() {
        let (tx, rx) = tokio::sync::watch::channel(false);
        let mut rx = rx;
        tokio::spawn(async move {
            // Emulates spawn_signal_watcher latching shutdown after a termination signal.
            let _ = tx.send(true);
        });
        // Must resolve promptly once the flag flips (fails-before: a hand-rolled signal future
        // would never see this in-process flip).
        tokio::time::timeout(Duration::from_secs(2), wait_for_shutdown(&mut rx))
            .await
            .expect("shutdown_signal did not resolve after the flag flipped");
    }

    #[tokio::test]
    async fn returns_immediately_if_already_shutting_down() {
        // A receiver cloned after the signal already fired must still observe the latched value.
        let (tx, rx) = tokio::sync::watch::channel(true);
        drop(tx); // all senders gone, but the latched `true` persists
        let mut rx = rx;
        tokio::time::timeout(Duration::from_millis(200), wait_for_shutdown(&mut rx))
            .await
            .expect("shutdown_signal should return immediately when already shutting down");
    }
}

#[cfg(test)]
mod reload_tests {
    //! Tests for [`apply_reloaded_config`]/[`reload_from_provider`] — the `reload-config`
    //! command action's plumbing (DESIGN-uns §9.5, [`commands::CommandInbox`]). In particular,
    //! these pin that BOTH the watch-driven hot-reload path and the pull-based `reload-config`
    //! path leave `config` (the single `ArcSwap` read by [`EdgeCommons::config`], the `cfg`
    //! publisher, and `get-configuration`) holding the freshly applied snapshot immediately on
    //! success — the historical Java `fullConfig`-staleness bug has no Rust analog because there
    //! is no second cached copy of the applied document to go stale; see the parity report.
    use super::*;
    use crate::config::source::ConfigSource;
    use crate::error::EdgeCommonsError;
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A [`ConfigSource`] whose `load()` replays a scripted sequence of results.
    struct FakeSource {
        results: StdMutex<Vec<Result<Value>>>,
    }

    impl FakeSource {
        fn new(results: Vec<Result<Value>>) -> Self {
            Self {
                results: StdMutex::new(results),
            }
        }
    }

    #[async_trait]
    impl ConfigSource for FakeSource {
        async fn load(&self) -> Result<Value> {
            let mut results = self.results.lock().unwrap();
            if results.is_empty() {
                Err(EdgeCommonsError::Config("FakeSource exhausted".to_string()))
            } else {
                results.remove(0)
            }
        }

        fn source_name(&self) -> &str {
            "FAKE"
        }
    }

    fn empty_listeners() -> ConfigListeners {
        Arc::new(std::sync::Mutex::new(Vec::new()))
    }

    struct CountingListener(Arc<AtomicUsize>);

    #[async_trait]
    impl config::ConfigurationChangeListener for CountingListener {
        async fn on_configuration_change(&self, _config: Arc<Config>) -> bool {
            self.0.fetch_add(1, Ordering::SeqCst);
            true
        }
    }

    #[tokio::test]
    async fn apply_reloaded_config_rejects_schema_invalid_and_keeps_previous() {
        let original = Arc::new(Config::from_value("C", "t", json!({ "component": {} })).unwrap());
        let config = Arc::new(ArcSwap::from(original.clone()));
        // No "component" key: fails the schema's `required: [component]`.
        let applied = apply_reloaded_config(
            json!({ "metricEmission": { "target": "nope" } }),
            &config,
            &empty_listeners(),
            "C",
            "t",
        )
        .await;
        assert!(!applied);
        assert!(
            Arc::ptr_eq(&config.load_full(), &original),
            "the previous config must be kept on validation failure"
        );
    }

    #[tokio::test]
    async fn apply_reloaded_config_stores_the_new_snapshot_and_notifies_listeners() {
        let original = Config::from_value("C", "t", json!({ "component": {} })).unwrap();
        let config = Arc::new(ArcSwap::from_pointee(original));
        let notified = Arc::new(AtomicUsize::new(0));
        let listeners = empty_listeners();
        listeners
            .lock()
            .unwrap()
            .push(Arc::new(CountingListener(notified.clone())) as _);

        let applied = apply_reloaded_config(
            json!({ "component": { "global": { "v": 2 } } }),
            &config,
            &listeners,
            "C",
            "t",
        )
        .await;

        assert!(applied);
        assert_eq!(
            notified.load(Ordering::SeqCst),
            1,
            "the listener must fire on a successful apply"
        );
        // The fullConfig-staleness check: the live snapshot (what get-configuration / the cfg
        // publisher read) reflects the reload immediately.
        assert_eq!(config.load_full().raw["component"]["global"]["v"], 2);
        assert_eq!(
            config::effective::redact(&config.load_full().raw)["component"]["global"]["v"],
            2,
            "the redacted snapshot get-configuration serves must also see the fresh value"
        );
    }

    #[tokio::test]
    async fn reload_from_provider_keeps_previous_on_fetch_failure() {
        let original = Arc::new(Config::from_value("C", "t", json!({ "component": {} })).unwrap());
        let config = Arc::new(ArcSwap::from(original.clone()));
        let source = FakeSource::new(vec![Err(EdgeCommonsError::Config(
            "broker down".to_string(),
        ))]);

        let ok = reload_from_provider(&source, &config, &empty_listeners(), "C", "t").await;

        assert!(!ok);
        assert!(Arc::ptr_eq(&config.load_full(), &original));
    }

    #[tokio::test]
    async fn reload_from_provider_keeps_previous_when_the_refetched_document_is_invalid() {
        let original = Arc::new(Config::from_value("C", "t", json!({ "component": {} })).unwrap());
        let config = Arc::new(ArcSwap::from(original.clone()));
        // Fetches successfully, but the document fails schema validation (no "component").
        let source = FakeSource::new(vec![Ok(json!({ "metricEmission": { "target": "nope" } }))]);

        let ok = reload_from_provider(&source, &config, &empty_listeners(), "C", "t").await;

        assert!(!ok);
        assert!(Arc::ptr_eq(&config.load_full(), &original));
    }

    #[tokio::test]
    async fn reload_from_provider_re_fetches_validates_and_applies_via_the_shared_apply_path() {
        let original =
            Config::from_value("C", "t", json!({ "component": { "global": { "v": 1 } } })).unwrap();
        let config = Arc::new(ArcSwap::from_pointee(original));
        let source = FakeSource::new(vec![Ok(json!({ "component": { "global": { "v": 99 } } }))]);

        let ok = reload_from_provider(&source, &config, &empty_listeners(), "C", "t").await;

        assert!(ok);
        // Same fullConfig-staleness guarantee as above, exercised through the reload-config
        // command's actual entry point (re-fetch -> validate -> apply -> notify).
        assert_eq!(config.load_full().raw["component"]["global"]["v"], 99);
    }
}
