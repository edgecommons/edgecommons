//! # DataFacade — the `data()` publish facade
//!
//! **One-liner purpose**: The telemetry/signal data-plane publish facade (DESIGN-class-facades
//! §2.1, D2/D5), mirroring the Java canonical `com.mbreissi.edgecommons.facades.DataFacade`.
//!
//! Constructs and validates the `SouthboundSignalUpdate` body (`device`/`signal`/`samples`) so an
//! adapter never hand-builds it, applies the body defaults, sanitizes the signal path into the
//! UNS `data` channel, stamps the envelope identity, and routes to the resolved [`Channel`]. It
//! publishes through the **ordinary, guarded** `messaging().publish(...)` — `data` is
//! non-reserved, so it passes the guard; the facade adds body-contract enforcement + defaults,
//! **not** privilege.
//!
//! ## Defaulting (DESIGN-class-facades §2.1, pinned by `uns-test-vectors/data.json`)
//! 1. `quality` → [`Quality::Good`] when omitted on a sample that carries a value.
//! 2. `qualityRaw` → the synthetic marker [`QUALITY_UNSPECIFIED`] when (and only when) the
//!    quality was defaulted; else the caller's value verbatim, else absent.
//! 3. `serverTs` → now (ISO-8601 UTC `…Z`, from the injected [`super::Clock`]) when omitted;
//!    `sourceTs` is **never** synthesized (absent when the source has none).
//! 4. The `samples` wrapper is enforced for the value-shorthand ([`DataFacade::publish_value`]) —
//!    a caller never emits a bare value.
//! 5. `signal.id` is the **only** hard reject — a publish with no stable id fails with
//!    [`crate::EdgeCommonsError::Facade`] at the call site.
//!
//! ## Channel routing (DESIGN-class-facades §4, D1)
//! Per-call [`SignalUpdate::via`] override ▸ config `publish.channel` (instance ▸ global) ▸
//! [`Channel::Local`]. A `stream:<name>` route serializes the same envelope and appends it to the
//! bound [`StreamSink`] (partition key = `signal.id`, ts = `serverTs`); when no sink is wired
//! (no `streaming` config section, or the `streaming` cargo feature is off) it falls back to a
//! LOCAL publish (readiness / no-streaming → local). Northbound / stream transport failures are
//! caught and logged — they must never flip local readiness.

use std::sync::Arc;

use serde_json::{Map, Value};

use crate::config::model::Config;
use crate::config::template::sanitize;
use crate::error::{EdgeCommonsError, Result};
use crate::messaging::message::{Message, MessageBuilder};
use crate::messaging::{MessagingService, Qos};
use crate::uns::{Uns, UnsClass};

use super::{Channel, Clock, Quality, Sample, SignalUpdate, SignalUpdateBuilder, StreamSink};

/// The signal-update envelope header name (`docs/SOUTHBOUND.md` §2).
pub const DATA_MESSAGE_NAME: &str = "SouthboundSignalUpdate";
/// The signal-update envelope header version.
pub const DATA_MESSAGE_VERSION: &str = "1.0";
/// The `qualityRaw` marker written when `quality` was defaulted to [`Quality::Good`].
pub const QUALITY_UNSPECIFIED: &str = "unspecified";

/// The `data()` publish facade bound to one instance — see the [module docs](self). Obtain via
/// [`crate::EdgeCommonsInstance::data`] (or the `main`-instance convenience [`crate::EdgeCommons::data`]).
pub struct DataFacade {
    config: Arc<Config>,
    instance_id: String,
    uns: Uns,
    messaging: Option<Arc<dyn MessagingService>>,
    stream_sink: Option<Arc<dyn StreamSink>>,
    clock: Clock,
}

impl DataFacade {
    /// Library-internal constructor (see the [`crate::EdgeCommonsInstance::data`] wiring).
    pub(crate) fn new(
        config: Arc<Config>,
        instance_id: String,
        uns: Uns,
        messaging: Option<Arc<dyn MessagingService>>,
        stream_sink: Option<Arc<dyn StreamSink>>,
        clock: Clock,
    ) -> DataFacade {
        DataFacade { config, instance_id, uns, messaging, stream_sink, clock }
    }

    // ===================== fluent builder entry point =====================

    /// Starts building a `SouthboundSignalUpdate` for a stable `signal.id` — the fluent body
    /// builder that subsumes the hand-assembled JSON body. Terminate with
    /// [`SignalUpdateBuilder::build`] and pass the result to [`Self::publish`].
    pub fn signal(&self, id: impl Into<String>) -> SignalUpdateBuilder {
        SignalUpdate::builder().signal_id(id)
    }

    // ===================== value shorthand =====================

    /// The value-shorthand: publish one value for a signal path (the path doubles as the stable
    /// `signal.id`). The single value is wrapped into a one-element `samples` array with
    /// `quality=GOOD`, `qualityRaw="unspecified"`, `serverTs=now` — a caller never emits a bare
    /// value.
    pub async fn publish_value(
        &self,
        signal_path: impl Into<String>,
        value: impl Into<Value>,
    ) -> Result<()> {
        let path = signal_path.into();
        let update = SignalUpdate::builder()
            .signal_id(path.clone())
            .sample(Sample::new(value))
            .signal_path(path)
            .build();
        self.publish(update).await
    }

    /// The value-shorthand with an explicit quality (so a source that knows the read is
    /// stale/failed marks it `BAD`/`UNCERTAIN`).
    pub async fn publish_value_with_quality(
        &self,
        signal_path: impl Into<String>,
        value: impl Into<Value>,
        quality: Quality,
    ) -> Result<()> {
        let path = signal_path.into();
        let update = SignalUpdate::builder()
            .signal_id(path.clone())
            .sample(Sample::with_quality(value, quality))
            .signal_path(path)
            .build();
        self.publish(update).await
    }

    // ===================== the raw escape hatch =====================

    /// The raw escape hatch (D5): publishes a caller-owned pre-built body verbatim to
    /// `data/{signal_path}`, applying **no** body defaulting — only the topic + identity
    /// guarantees. For a component with an exotic body the facade should not shape.
    pub async fn publish_body(&self, signal_path: &str, body: Value) -> Result<()> {
        self.publish_body_via(signal_path, body, None).await
    }

    /// [`Self::publish_body`] with an explicit [`Channel`] override.
    pub async fn publish_body_via(
        &self,
        signal_path: &str,
        body: Value,
        via: Option<Channel>,
    ) -> Result<()> {
        let channel = self.channel_token(signal_path)?;
        let topic = self.uns.topic_with_channel(UnsClass::Data, &channel)?;
        let msg = self.message(body.clone());
        let ts_millis = first_server_ts_millis(&body);
        self.route(via, &topic, msg, signal_path, ts_millis).await
    }

    // ===================== the SignalUpdate publish path =====================

    /// Publishes a built [`SignalUpdate`]: validates `signal.id`, constructs the body with the
    /// defaulting rules, sanitizes the path into the `data` channel, stamps the envelope, and
    /// routes to the resolved channel.
    ///
    /// # Errors
    /// [`EdgeCommonsError::Facade`] when `signal.id` is missing/empty, `samples` is empty, or a sample
    /// carries no value; [`EdgeCommonsError::UnsValidation`] on a bad channel token;
    /// [`EdgeCommonsError::Messaging`] when no messaging transport is wired.
    pub async fn publish(&self, update: SignalUpdate) -> Result<()> {
        let signal_id = update
            .signal_id
            .clone()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                EdgeCommonsError::Facade(
                    "data publish requires a stable signal.id (the consumer key) - it is the \
                     only non-defaultable field"
                        .to_string(),
                )
            })?;
        if update.samples.is_empty() {
            return Err(EdgeCommonsError::Facade("data publish requires at least one sample".to_string()));
        }
        let body = self.build_body(&update)?;
        let path = update.effective_signal_path().unwrap_or(&signal_id).to_string();
        let channel = self.channel_token(&path)?;
        let topic = self.uns.topic_with_channel(UnsClass::Data, &channel)?;
        let msg = self.message(body.clone());
        let ts_millis = first_server_ts_millis(&body);
        self.route(update.via.clone(), &topic, msg, &signal_id, ts_millis).await
    }

    // ===================== body construction (THE contract) =====================

    /// Constructs the wire body from a [`SignalUpdate`], applying the §2.1 defaulting rules
    /// (quality → GOOD + `qualityRaw` marker, `serverTs` → now, samples wrapper). Deterministic
    /// given the injected clock — this is the exact body the vectors pin.
    ///
    /// # Errors
    /// [`EdgeCommonsError::Facade`] when a sample carries no value.
    pub fn build_body(&self, update: &SignalUpdate) -> Result<Value> {
        let mut signal = Map::new();
        signal.insert(
            "id".to_string(),
            Value::String(update.signal_id.clone().unwrap_or_default()),
        );
        if let Some(name) = &update.signal_name {
            signal.insert("name".to_string(), Value::String(name.clone()));
        }
        if let Some(address) = &update.signal_address {
            signal.insert("address".to_string(), address.clone());
        }

        let mut samples = Vec::with_capacity(update.samples.len());
        for sample in &update.samples {
            samples.push(self.build_sample(sample)?);
        }

        let mut body = Map::new();
        if let Some(device) = &update.device {
            body.insert("device".to_string(), device.clone());
        }
        body.insert("signal".to_string(), Value::Object(signal));
        body.insert("samples".to_string(), Value::Array(samples));
        Ok(Value::Object(body))
    }

    /// Builds one sample with the quality/qualityRaw/serverTs defaulting rules.
    fn build_sample(&self, sample: &Sample) -> Result<Value> {
        let value = sample.value.clone().ok_or_else(|| {
            EdgeCommonsError::Facade(
                "data sample value is required (a quality-only sample is not a sample) - pass \
                 BAD/UNCERTAIN for a failed read"
                    .to_string(),
            )
        })?;
        let mut out = Map::new();
        out.insert("value".to_string(), value);

        let quality_defaulted = sample.quality.is_none();
        let quality = sample.quality.unwrap_or(Quality::Good);
        out.insert("quality".to_string(), Value::String(quality.wire().to_string()));

        let quality_raw = match (&sample.quality_raw, quality_defaulted) {
            (Some(raw), _) => Some(raw.clone()),
            (None, true) => Some(QUALITY_UNSPECIFIED.to_string()),
            (None, false) => None,
        };
        if let Some(raw) = quality_raw {
            out.insert("qualityRaw".to_string(), Value::String(raw));
        }

        if let Some(source_ts) = &sample.source_ts {
            out.insert("sourceTs".to_string(), Value::String(source_ts.clone()));
        }
        let server_ts = sample.server_ts.clone().unwrap_or_else(|| (self.clock)());
        out.insert("serverTs".to_string(), Value::String(server_ts));
        Ok(Value::Object(out))
    }

    // ===================== channel routing =====================

    /// Resolves the effective channel: per-call `via` override ▸ config `publish.channel`
    /// (instance ▸ global) ▸ [`Channel::Local`] (DESIGN-class-facades §4, D1).
    pub fn resolve_channel(&self, via: Option<Channel>) -> Channel {
        via.or_else(|| self.configured_channel()).unwrap_or(Channel::Local)
    }

    /// Reads the config `publish.channel` default (Option C): the bound instance's
    /// `publish.channel` ▸ the global `component.global.publish.channel`. Best-effort — any
    /// lookup/parse anomaly yields `None` (fall through to LOCAL).
    fn configured_channel(&self) -> Option<Channel> {
        publish_channel_of(self.config.instance(&self.instance_id))
            .or_else(|| publish_channel_of(Some(self.config.global())))
    }

    /// Routes a built envelope to the resolved channel. LOCAL publishes on the guarded bus;
    /// NORTHBOUND publishes to IoT Core; a stream route appends the serialized envelope to the
    /// named stream (falling back to LOCAL when no sink is wired). Northbound / stream failures
    /// are caught + logged (they must never flip local readiness).
    async fn route(
        &self,
        via: Option<Channel>,
        topic: &str,
        msg: Message,
        partition_key: &str,
        ts_millis: u64,
    ) -> Result<()> {
        match self.resolve_channel(via) {
            Channel::Local => self.messaging()?.publish(topic, &msg).await,
            Channel::Northbound => {
                let messaging = self.messaging()?;
                if let Err(e) = messaging.publish_to_iot_core(topic, &msg, Qos::AtLeastOnce).await
                {
                    tracing::warn!(
                        topic,
                        error = %e,
                        "northbound data publish failed (local readiness unaffected)"
                    );
                }
                Ok(())
            }
            Channel::Stream(name) => {
                self.append_to_stream(&name, topic, &msg, partition_key, ts_millis).await
            }
        }
    }

    /// The `stream:<name>` route: append the serialized envelope, or fall back to LOCAL.
    async fn append_to_stream(
        &self,
        stream_name: &str,
        topic: &str,
        msg: &Message,
        partition_key: &str,
        ts_millis: u64,
    ) -> Result<()> {
        match &self.stream_sink {
            None => {
                tracing::warn!(
                    stream = stream_name,
                    "data channel 'stream:{stream_name}' requested but streaming is not \
                     configured - routing to LOCAL instead (readiness/no-streaming -> local)"
                );
                self.messaging()?.publish(topic, msg).await
            }
            Some(sink) => {
                let payload = msg.to_vec()?;
                if let Err(e) = sink.append(stream_name, partition_key, ts_millis, payload) {
                    tracing::warn!(
                        stream = stream_name,
                        error = %e,
                        "stream append failed (local readiness unaffected)"
                    );
                }
                Ok(())
            }
        }
    }

    // ===================== helpers =====================

    /// The sanitized channel token for a signal path (each `/`-token → a UNS token).
    ///
    /// # Errors
    /// [`EdgeCommonsError::Facade`] when `signal_path` is empty.
    pub fn channel_token(&self, signal_path: &str) -> Result<String> {
        if signal_path.is_empty() {
            return Err(EdgeCommonsError::Facade("data signal path must be non-empty".to_string()));
        }
        Ok(signal_path.split('/').map(sanitize).collect::<Vec<_>>().join("/"))
    }

    /// Builds the identity-stamped envelope with the signal-update header.
    fn message(&self, body: Value) -> Message {
        MessageBuilder::new(DATA_MESSAGE_NAME, DATA_MESSAGE_VERSION)
            .from_config(&self.config)
            .instance(self.instance_id.clone())
            .payload(body)
            .build()
    }

    fn messaging(&self) -> Result<&Arc<dyn MessagingService>> {
        self.messaging.as_ref().ok_or_else(|| {
            EdgeCommonsError::Messaging(
                "messaging is not available: data() requires a wired messaging transport"
                    .to_string(),
            )
        })
    }

    /// The instance token this facade is bound to.
    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }
}

/// `section.publish.channel` as a [`Channel`], or `None` when absent/unparseable.
fn publish_channel_of(section: Option<&Value>) -> Option<Channel> {
    let publish = section?.get("publish")?.as_object()?;
    let channel = publish.get("channel")?.as_str()?;
    Channel::from_config(channel)
}

/// The first sample's `serverTs` parsed to epoch millis (the stream record timestamp), or "now"
/// when absent/unparseable.
fn first_server_ts_millis(body: &Value) -> u64 {
    body.get("samples")
        .and_then(Value::as_array)
        .and_then(|samples| samples.first())
        .and_then(|first| first.get("serverTs"))
        .and_then(Value::as_str)
        .and_then(parse_rfc3339_millis)
        .unwrap_or_else(now_millis)
}

/// Parses an RFC3339 timestamp to epoch millis, or `None` on a malformed string.
fn parse_rfc3339_millis(ts: &str) -> Option<u64> {
    let parsed =
        time::OffsetDateTime::parse(ts, &time::format_description::well_known::Rfc3339).ok()?;
    u64::try_from(parsed.unix_timestamp_nanos() / 1_000_000).ok()
}

/// The current epoch time in millis (fallback when a `serverTs` is absent/unparseable).
fn now_millis() -> u64 {
    let nanos = time::OffsetDateTime::now_utc().unix_timestamp_nanos();
    u64::try_from(nanos / 1_000_000).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::RecordingMessaging;
    use serde_json::json;
    use std::sync::Mutex;

    fn fixed_clock() -> Clock {
        Arc::new(|| "2026-07-01T12:00:00Z".to_string())
    }

    fn test_config() -> Arc<Config> {
        Arc::new(Config::from_value("opcua-adapter", "gw-01", json!({})).unwrap())
    }

    fn facade(messaging: Arc<RecordingMessaging>) -> DataFacade {
        let config = test_config();
        let uns = Uns::new(config.identity().with_instance("kep1").unwrap(), false);
        DataFacade::new(
            config,
            "kep1".to_string(),
            uns,
            Some(messaging as Arc<dyn MessagingService>),
            None,
            fixed_clock(),
        )
    }

    #[tokio::test]
    async fn value_shorthand_defaults_quality_and_server_ts() {
        let messaging = RecordingMessaging::new();
        let f = facade(messaging.clone());
        f.publish_value("temp", 21.5).await.unwrap();
        let (topic, msg) = &messaging.local()[0];
        assert_eq!(topic, "ecv1/gw-01/opcua-adapter/kep1/data/temp");
        assert_eq!(msg.header.name, DATA_MESSAGE_NAME);
        assert_eq!(msg.body["signal"]["id"], "temp");
        assert_eq!(msg.body["samples"][0]["value"], 21.5);
        assert_eq!(msg.body["samples"][0]["quality"], "GOOD");
        assert_eq!(msg.body["samples"][0]["qualityRaw"], "unspecified");
        assert_eq!(msg.body["samples"][0]["serverTs"], "2026-07-01T12:00:00Z");
    }

    #[tokio::test]
    async fn explicit_quality_is_not_marked_unspecified() {
        let messaging = RecordingMessaging::new();
        let f = facade(messaging.clone());
        f.publish_value_with_quality("temp", 0, Quality::Bad).await.unwrap();
        let (_, msg) = &messaging.local()[0];
        assert_eq!(msg.body["samples"][0]["quality"], "BAD");
        assert!(msg.body["samples"][0].get("qualityRaw").is_none());
    }

    #[tokio::test]
    async fn missing_signal_id_is_rejected() {
        let messaging = RecordingMessaging::new();
        let f = facade(messaging);
        let update = SignalUpdate::builder().sample(Sample::new(1)).build();
        assert!(matches!(f.publish(update).await, Err(EdgeCommonsError::Facade(_))));
    }

    #[tokio::test]
    async fn empty_samples_is_rejected() {
        let messaging = RecordingMessaging::new();
        let f = facade(messaging);
        let update = SignalUpdate::builder().signal_id("temp").build();
        assert!(matches!(f.publish(update).await, Err(EdgeCommonsError::Facade(_))));
    }

    #[tokio::test]
    async fn quality_only_sample_is_rejected() {
        let messaging = RecordingMessaging::new();
        let f = facade(messaging);
        let update = SignalUpdate::builder()
            .signal_id("temp")
            .sample(Sample { quality: Some(Quality::Bad), ..Default::default() })
            .build();
        assert!(matches!(f.publish(update).await, Err(EdgeCommonsError::Facade(_))));
    }

    #[tokio::test]
    async fn channel_path_is_sanitized() {
        let messaging = RecordingMessaging::new();
        let f = facade(messaging.clone());
        f.publish(f.signal("s2").sample(Sample::new(1.0)).signal_path("a+b").build())
            .await
            .unwrap();
        let (topic, _) = &messaging.local()[0];
        assert_eq!(topic, "ecv1/gw-01/opcua-adapter/kep1/data/a_b");
    }

    #[tokio::test]
    async fn northbound_override_routes_to_iot_core() {
        let messaging = RecordingMessaging::new();
        let f = facade(messaging.clone());
        f.publish(f.signal("temp").sample(Sample::new(1.0)).via(Channel::Northbound).build())
            .await
            .unwrap();
        assert!(messaging.local().is_empty());
        assert_eq!(messaging.iot().len(), 1);
    }

    #[tokio::test]
    async fn stream_route_appends_to_the_sink() {
        /// `(stream_name, partition_key, timestamp_ms, payload)`.
        type Recorded = (String, String, u64, Vec<u8>);
        struct Recorder(Mutex<Option<Recorded>>);
        impl StreamSink for Recorder {
            fn append(
                &self,
                stream_name: &str,
                partition_key: &str,
                timestamp_ms: u64,
                payload: Vec<u8>,
            ) -> Result<()> {
                *self.0.lock().unwrap() =
                    Some((stream_name.to_string(), partition_key.to_string(), timestamp_ms, payload));
                Ok(())
            }
        }
        let messaging = RecordingMessaging::new();
        let sink = Arc::new(Recorder(Mutex::new(None)));
        let config = test_config();
        let uns = Uns::new(config.identity().with_instance("kep1").unwrap(), false);
        let f = DataFacade::new(
            config,
            "kep1".to_string(),
            uns,
            Some(messaging.clone() as Arc<dyn MessagingService>),
            Some(sink.clone() as Arc<dyn StreamSink>),
            fixed_clock(),
        );
        f.publish(
            f.signal("temp").sample(Sample::new(21.5)).via(Channel::stream("hot").unwrap()).build(),
        )
        .await
        .unwrap();
        assert!(messaging.local().is_empty(), "the stream route must not also publish locally");
        let recorded = sink.0.lock().unwrap().take().expect("stream append recorded");
        assert_eq!(recorded.0, "hot");
        assert_eq!(recorded.1, "temp");
    }

    #[tokio::test]
    async fn stream_route_falls_back_to_local_when_no_sink_is_wired() {
        let messaging = RecordingMessaging::new();
        let f = facade(messaging.clone()); // built with stream_sink = None
        f.publish(
            f.signal("temp").sample(Sample::new(21.5)).via(Channel::stream("hot").unwrap()).build(),
        )
        .await
        .unwrap();
        assert_eq!(messaging.local().len(), 1, "no sink wired -> LOCAL fallback (D1a)");
    }

    #[tokio::test]
    async fn raw_escape_hatch_publishes_the_body_verbatim() {
        let messaging = RecordingMessaging::new();
        let f = facade(messaging.clone());
        f.publish_body("temp", json!({ "custom": true })).await.unwrap();
        let (topic, msg) = &messaging.local()[0];
        assert_eq!(topic, "ecv1/gw-01/opcua-adapter/kep1/data/temp");
        assert_eq!(msg.body, json!({ "custom": true }));
    }
}
