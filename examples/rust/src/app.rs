//! # Skeleton application logic
//!
//! **One-liner purpose**: The component's business logic, demonstrating the
//! `ggcommons` messaging, metrics, configuration, and lifecycle features.
//!
//! ## Overview
//! [`SkeletonApp`] wires the concerns that every real component needs:
//! 1. **Request/reply** — subscribes to a request topic and replies to each request.
//! 2. **Periodic publish** — publishes a data message on an interval read from
//!    configuration (`component.global.publish_interval`), emitting a metric each time.
//! 3. **Dynamic config** — registers a [`ConfigurationChangeListener`] so the publish
//!    interval updates live on a config hot-reload, without a restart.
//! 4. **Graceful shutdown** — runs until Ctrl-C / SIGTERM, then unsubscribes and
//!    returns so the [`ggcommons::GgCommons`] runtime can drop cleanly (RAII).
//!
//! Messaging is available in both STANDALONE and GREENGRASS mode (the latter with
//! the `greengrass` feature). If a build omits a messaging transport, the app
//! degrades to heartbeat-only operation and simply waits for shutdown.
//!
//! ## Semantics & Architecture
//! - Async (`tokio`); the app holds cloned `Arc` service handles, never the runtime.
//! - Error handling: [`anyhow::Result`] at this (binary) layer.
//!
//! ## Related Modules
//! - [`crate::main`] constructs and runs this app.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use ggcommons::messaging::message::MessageBuilder;
use ggcommons::prelude::*;
use serde_json::json;

/// Default publish interval (seconds) when `component.global.publish_interval` is absent.
const DEFAULT_PUBLISH_INTERVAL_SECS: u64 = 3;
/// Subscription queue depth for the request topic.
const REQUEST_QUEUE_SIZE: usize = 16;
/// Handler concurrency for the request topic (`1` = serial, ordered).
const REQUEST_CONCURRENCY: usize = 1;
/// The metric emitted on each periodic publish.
const PUBLISH_METRIC: &str = "messages_published";

/// The component's business logic and the service handles it operates over.
pub struct SkeletonApp {
    config: Arc<Config>,
    metrics: Arc<dyn MetricService>,
    /// `None` only when the build provides no messaging transport for the mode.
    messaging: Option<Arc<dyn MessagingService>>,
    /// Live publish interval (seconds), updated by [`IntervalListener`] on config
    /// hot-reload so the running loops pick up changes without a restart.
    publish_interval: Arc<AtomicU64>,
    /// Handle to the `telemetry` durable stream when the `streaming` feature is built and a
    /// `streaming` config section is present; `None` otherwise. The publish loop appends each
    /// data point here, and the library's export engine drains it to the configured sink
    /// (Kinesis on-device via TES). Cheap to clone; shared across threads.
    #[cfg(feature = "streaming")]
    stream: Option<StreamHandle>,
    /// The credential service when the `credentials` feature is built and a `credentials` config
    /// section is present; `None` otherwise. Demonstrates encrypted-vault secret access (and, with
    /// `credentials-aws` + a `central` config, sync from AWS Secrets Manager over TES).
    #[cfg(feature = "credentials")]
    credentials: Option<Arc<dyn ggcommons::credentials::CredentialService>>,
}

/// Config key (under `component.global`) naming the secret the component reads; the default is a
/// self-seeded demo secret so the example runs with no external provisioning.
#[cfg(feature = "credentials")]
const DEMO_SECRET_KEY: &str = "demo_secret";
/// Default secret name when `component.global.demo_secret` is absent.
#[cfg(feature = "credentials")]
const DEFAULT_DEMO_SECRET: &str = "skeleton/demo-secret";

/// The publish interval (seconds) from `component.global.publish_interval`,
/// falling back to [`DEFAULT_PUBLISH_INTERVAL_SECS`].
///
/// Greengrass stores configuration numbers as doubles, so a value like `5` comes
/// back as `5.0` — accept either an integer or a float JSON number.
fn interval_from(config: &Config) -> u64 {
    config
        .global()
        .get("publish_interval")
        .and_then(|v| v.as_u64().or_else(|| v.as_f64().map(|f| f as u64)))
        .unwrap_or(DEFAULT_PUBLISH_INTERVAL_SECS)
}

/// A [`ConfigurationChangeListener`] that refreshes the live publish interval when
/// the component configuration is hot-reloaded — the Rust counterpart of the Python
/// skeleton's `on_configuration_change` override (dynamic config pickup).
struct IntervalListener {
    publish_interval: Arc<AtomicU64>,
}

#[async_trait::async_trait]
impl ConfigurationChangeListener for IntervalListener {
    async fn on_configuration_change(&self, config: Arc<Config>) -> bool {
        let secs = interval_from(&config);
        self.publish_interval.store(secs, Ordering::Relaxed);
        tracing::info!(publish_interval = secs, "configuration changed; updated publish interval");
        true
    }
}

impl SkeletonApp {
    /// Build the app from an initialized [`ggcommons::GgCommons`] runtime.
    ///
    /// # Purpose
    /// Capture the service handles (config snapshot, metrics, messaging) the app
    /// needs, defining the metrics it will emit.
    ///
    /// # Post-conditions
    /// The `messages_published` metric is registered; `messaging` is `Some` in
    /// STANDALONE mode and `None` in GREENGRASS mode.
    ///
    /// # Errors
    /// Currently infallible, but returns `Result` so future wiring can fail cleanly.
    pub fn new(gg: &GgCommons) -> anyhow::Result<Self> {
        let metrics = gg.metrics();
        metrics.define_metric(
            MetricBuilder::create(PUBLISH_METRIC)
                .add_measure("count", "Count", 60)
                .build(),
        );

        let config = gg.config();
        let publish_interval = Arc::new(AtomicU64::new(interval_from(&config)));
        // Register for config hot-reload so the publish cadence tracks
        // `component.global.publish_interval` without a restart.
        gg.add_config_change_listener(Arc::new(IntervalListener {
            publish_interval: publish_interval.clone(),
        }));

        // Telemetry streaming (feature-gated): grab a handle to the `telemetry` stream the
        // library opened from the config's `streaming` section. Absent section -> no streams ->
        // run without streaming (the data is still published over MQTT as before).
        #[cfg(feature = "streaming")]
        let stream = {
            let streams = gg.streams();
            match streams.stream_names().is_empty() {
                true => {
                    tracing::info!("no `streaming` config section; telemetry streaming disabled");
                    None
                }
                false => match streams.stream("telemetry") {
                    Ok(handle) => {
                        tracing::info!(streams = ?streams.stream_names(), "telemetry streaming enabled");
                        Some(handle)
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "stream 'telemetry' not configured; streaming disabled");
                        None
                    }
                },
            }
        };

        Ok(Self {
            config,
            metrics,
            messaging: gg.messaging().ok(),
            publish_interval,
            #[cfg(feature = "streaming")]
            stream,
            #[cfg(feature = "credentials")]
            credentials: gg.credentials(),
        })
    }

    /// Demonstrate encrypted-vault secret access via `gg.credentials()`.
    ///
    /// # Purpose
    /// Show the credential-service usage every real component needs: read a named secret from the
    /// encrypted local vault and use it — without ever logging the value. Runs once at startup.
    ///
    /// In production the secret arrives via central sync (AWS Secrets Manager over TES, with a
    /// `credentials.central` config) or out-of-band provisioning; here, so the example is
    /// self-contained, we seed a demo value locally on first run if it is absent.
    ///
    /// # Errors
    /// Non-fatal: any vault error is logged and swallowed so the demo never takes the component down.
    #[cfg(feature = "credentials")]
    fn demonstrate_credentials(&self) {
        use ggcommons::credentials::PutOptions;

        let Some(creds) = &self.credentials else {
            tracing::info!("no `credentials` config section; secret access demo disabled");
            return;
        };
        let name = self
            .config
            .global()
            .get(DEMO_SECRET_KEY)
            .and_then(|v| v.as_str())
            .unwrap_or(DEFAULT_DEMO_SECRET)
            .to_string();

        // Seed a demo secret on first run (in production this comes from central sync/provisioning).
        match creds.exists(&name) {
            Ok(false) => {
                let demo = serde_json::json!({ "username": "svc-account", "password": "demo-secret-value" });
                let bytes = serde_json::to_vec(&demo).unwrap_or_default();
                match creds.put(&name, &bytes, PutOptions::default()) {
                    Ok(version) => tracing::info!(secret = %name, version = %version,
                        "seeded demo secret (production: provided via central sync / provisioning)"),
                    Err(e) => {
                        tracing::warn!(error = %e, secret = %name, "failed to seed demo secret");
                        return;
                    }
                }
            }
            Ok(true) => {}
            Err(e) => {
                tracing::warn!(error = %e, secret = %name, "vault unavailable; skipping secret demo");
                return;
            }
        }

        // Read it back and use it — logging only non-sensitive facts, never the value.
        match creds.get(&name) {
            Ok(Some(secret)) => {
                tracing::info!(
                    secret = %name,
                    bytes = secret.bytes().len(),
                    source = %secret.source,
                    "credential access OK (value redacted)"
                );
                // A real component would now use the secret (e.g. authenticate a downstream client).
                // Demonstrate a typed view; log only the non-secret username.
                match creds.get_basic_auth(&name) {
                    Ok(Some(ba)) => tracing::info!(secret = %name, username = %ba.username,
                        "parsed basic-auth view (password redacted)"),
                    Ok(None) => {}
                    Err(e) => tracing::debug!(error = %e, "secret is not a basic-auth JSON shape"),
                }
            }
            Ok(None) => tracing::warn!(secret = %name, "secret not found after seeding (unexpected)"),
            Err(e) => tracing::warn!(error = %e, secret = %name, "failed to read secret"),
        }
    }

    /// Run the component until a shutdown signal is received.
    ///
    /// # Purpose
    /// Start the request responder and the periodic publisher (when messaging is
    /// available), then block on the shutdown signal and tear down cleanly.
    ///
    /// # Post-conditions
    /// On return, the request subscription has been unsubscribed at the broker.
    ///
    /// # Errors
    /// Propagates failures from subscribing, publishing, or signal handling.
    pub async fn run(&self) -> anyhow::Result<()> {
        let thing = &self.config.thing_name;

        // Demonstrate encrypted-vault secret access once at startup (feature-gated, non-fatal).
        #[cfg(feature = "credentials")]
        self.demonstrate_credentials();

        let Some(messaging) = self.messaging.clone() else {
            tracing::warn!(
                "messaging unavailable for this build/mode; running heartbeat-only until shutdown"
            );
            shutdown_signal().await;
            return Ok(());
        };

        let request_topic = format!("{thing}/skeleton/request");
        let data_topic = format!("{thing}/skeleton/data");
        let cmd_topic = format!("{thing}/skeleton/cmd");
        let telemetry_topic = format!("{thing}/skeleton/telemetry");

        // 1. Respond to requests on the request topic (local pub/sub).
        let responder = messaging.clone();
        let responder_thing = thing.clone();
        messaging
            .subscribe(
                &request_topic,
                message_handler(move |topic, msg| {
                    let responder = responder.clone();
                    let responder_thing = responder_thing.clone();
                    async move {
                        tracing::info!(topic = %topic, request = %msg.header.name, "received request");
                        let reply = MessageBuilder::new("SkeletonReply", "1.0")
                            .thing_name(&responder_thing)
                            .payload(json!({ "echo": msg.body, "ok": true }))
                            .build();
                        if let Err(e) = responder.reply(&msg, reply).await {
                            tracing::warn!(error = %e, "failed to send reply");
                        }
                    }
                }),
                REQUEST_QUEUE_SIZE,
                REQUEST_CONCURRENCY,
            )
            .await?;
        tracing::info!(topic = %request_topic, "subscribed for requests");

        // 2. Subscribe to commands from AWS IoT Core (the IoT Core bridge); ack each
        //    one back to IoT Core (exercises subscribe_to_iot_core + publish_to_iot_core).
        //    Non-fatal: builds/modes without an IoT Core transport (e.g. local-only STANDALONE)
        //    simply skip the command bridge instead of failing the whole component — matching the
        //    already-non-fatal publish_to_iot_core in the publish loop.
        let acker = messaging.clone();
        let ack_thing = thing.clone();
        let ack_topic = telemetry_topic.clone();
        let iot_core_subscribed = messaging
            .subscribe_to_iot_core(
                &cmd_topic,
                message_handler(move |topic, msg| {
                    let acker = acker.clone();
                    let ack_thing = ack_thing.clone();
                    let ack_topic = ack_topic.clone();
                    async move {
                        tracing::info!(topic = %topic, "received IoT Core command");
                        let ack = MessageBuilder::new("CmdAck", "1.0")
                            .thing_name(&ack_thing)
                            .payload(json!({ "ack": msg.body }))
                            .build();
                        if let Err(e) = acker.publish_to_iot_core(&ack_topic, &ack, Qos::AtLeastOnce).await {
                            tracing::warn!(error = %e, "failed to ack IoT Core command");
                        }
                    }
                }),
                Qos::AtLeastOnce,
                REQUEST_QUEUE_SIZE,
                REQUEST_CONCURRENCY,
            )
            .await
            .map(|()| true)
            .unwrap_or_else(|e| {
                tracing::warn!(error = %e, topic = %cmd_topic, "IoT Core unavailable; skipping command bridge");
                false
            });
        if iot_core_subscribed {
            tracing::info!(topic = %cmd_topic, "subscribed to IoT Core commands");
        }

        // 3. Run the periodic publisher and the periodic self-requester until shutdown.
        tokio::select! {
            result = self.publish_loop(messaging.clone(), data_topic, telemetry_topic) => result?,
            result = self.request_loop(messaging.clone(), request_topic.clone()) => result?,
            _ = shutdown_signal() => tracing::info!("shutdown signal received"),
        }

        // 4. Clean up subscriptions before the runtime drops (only the ones established).
        messaging.unsubscribe(&request_topic).await?;
        if iot_core_subscribed {
            messaging.unsubscribe_from_iot_core(&cmd_topic).await?;
        }
        Ok(())
    }

    /// Periodically issue a request to our own request topic and log the reply,
    /// demonstrating request/reply correlation end-to-end over the transport.
    ///
    /// # Errors
    /// Returns on a fatal send failure; per-attempt timeouts/errors are logged.
    async fn request_loop(
        &self,
        messaging: Arc<dyn MessagingService>,
        request_topic: String,
    ) -> anyhow::Result<()> {
        loop {
            tokio::time::sleep(Duration::from_secs(self.publish_interval().max(1) * 2)).await;
            let request = MessageBuilder::new("SkeletonRequest", "1.0")
                .from_config(&self.config)
                .payload(json!({ "ping": true }))
                .build();
            match messaging.request(&request_topic, request).await {
                Ok(reply_future) => {
                    match tokio::time::timeout(Duration::from_secs(10), reply_future).await {
                        Ok(Ok(reply)) => {
                            tracing::info!(reply = %reply.header.name, body = %reply.body, "request/reply round-trip OK")
                        }
                        Ok(Err(e)) => tracing::warn!(error = %e, "reply was an error"),
                        Err(_) => tracing::warn!("request timed out"),
                    }
                }
                Err(e) => tracing::warn!(error = %e, "request failed to send"),
            }
        }
    }

    /// Publish a data message on the configured interval, emitting one metric per send.
    ///
    /// # Purpose
    /// Demonstrate config-driven periodic publishing plus metric emission. Reads the
    /// interval from the live config snapshot so a hot reload of
    /// `component.global.publish_interval` takes effect on the next tick.
    ///
    /// # Errors
    /// Returns on the first publish failure (the caller decides recovery).
    async fn publish_loop(
        &self,
        messaging: Arc<dyn MessagingService>,
        topic: String,
        telemetry_topic: String,
    ) -> anyhow::Result<()> {
        let mut seq: u64 = 0;
        loop {
            let interval = self.publish_interval();
            tokio::time::sleep(Duration::from_secs(interval)).await;

            seq += 1;
            let msg = MessageBuilder::new("SkeletonData", "1.0")
                .from_config(&self.config)
                .payload(json!({ "seq": seq }))
                .build();
            messaging.publish(&topic, &msg).await?;
            // Also mirror to AWS IoT Core (exercises the IoT Core bridge / publish_to_iot_core).
            if let Err(e) = messaging
                .publish_to_iot_core(&telemetry_topic, &msg, Qos::AtLeastOnce)
                .await
            {
                tracing::warn!(error = %e, "failed to publish telemetry to IoT Core");
            }
            tracing::info!(topic = %topic, seq, "published data message");

            // Also append the data point to the durable telemetry stream (feature-gated). The
            // library's export engine drains it to the configured sink (Kinesis via TES on-device)
            // independently — append returns once the record is committed to the local buffer, so a
            // sink/network outage never blocks the publish loop. Partition by Thing for ordered
            // per-device delivery downstream.
            #[cfg(feature = "streaming")]
            if let Some(stream) = &self.stream {
                let payload = serde_json::to_vec(&json!({ "seq": seq, "thing": self.config.thing_name }))
                    .unwrap_or_default();
                let record = StreamRecord::new(self.config.thing_name.clone(), now_ms(), payload);
                match stream.append(record) {
                    Ok(()) => tracing::debug!(seq, "appended record to telemetry stream"),
                    Err(e) => tracing::warn!(error = %e, "failed to append to telemetry stream"),
                }
            }

            let mut values = std::collections::HashMap::new();
            values.insert("count".to_string(), 1.0);
            if let Err(e) = self.metrics.emit_metric(PUBLISH_METRIC, values).await {
                tracing::warn!(error = %e, "failed to emit publish metric");
            }
        }
    }

    /// The current publish interval in seconds (≥1), read from the live value that
    /// [`IntervalListener`] refreshes on config hot-reload.
    fn publish_interval(&self) -> u64 {
        self.publish_interval.load(Ordering::Relaxed).max(1)
    }
}

/// Current Unix time in milliseconds, used as the telemetry record timestamp. Falls back to
/// `0` if the system clock is before the epoch (which would also break TLS, so never in practice).
#[cfg(feature = "streaming")]
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Resolve when the process should shut down: Ctrl-C on all platforms, plus SIGTERM
/// on Unix (the signal Greengrass sends to stop a component).
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = term.recv() => {},
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
