//! # Facades — the app-usable class publish facades (`data()`/`events()`/`app()`)
//!
//! **One-liner purpose**: `data()`/`events()`/`app()` — the three **non-reserved**, app-usable
//! UNS class publish facades (`docs/platform/DESIGN-class-facades.md`), mirroring the Java
//! canonical `com.mbreissi.edgecommons.facades` package.
//!
//! ## Overview
//! Every one of the eight UNS classes now has exactly one library-owned owner: the reserved
//! classes (`state`/`metric`/`cfg`/`log`) are published through the privileged
//! `ReservedMessaging` seam; the app-usable classes (`data`/`evt`/`app`) are published through the
//! **ordinary, guarded** [`crate::messaging::MessagingService`] — they pass the reserved-class
//! guard because their class is not reserved. The facades add **no** new privilege; they add
//! **body-contract enforcement + sane defaults + one obvious call site**, replacing a hand-rolled
//! raw `uns().topic(...)` + hand-built body + `messaging().publish(...)` ritual (the drift
//! `docs/platform/DESIGN-class-facades.md` §1 documents).
//!
//! - [`DataFacade`] (`data`) — the telemetry/signal data plane. Constructs the
//!   `SouthboundSignalUpdate` body; `quality` defaults to [`Quality::Good`] (with a
//!   `qualityRaw:"unspecified"` marker) and `serverTs` to now when omitted; the value-shorthand
//!   wraps a bare value into the one-element `samples` array. The only hard reject is a
//!   missing/empty `signal.id`. Channel routing (§4, D1): per-call override ▸ config
//!   `publish.channel` ▸ [`Channel::Local`]; a `stream:<name>` route composes the bound
//!   [`StreamSink`] and falls back to LOCAL when none is wired.
//! - [`EventsFacade`] (`evt`) — operator events & alarms. The `evt/{severity}/{type}` channel is
//!   **derived from the body's own severity + type** (topic and body can never disagree);
//!   `timestamp` defaults to now; `raise_alarm`/`clear_alarm` set `alarm`/`active` and default
//!   severity to [`Severity::Critical`]. LOCAL/NORTHBOUND only (no stream route).
//! - [`AppFacade`] (`app`) — free-form inter-component pub/sub. A named header + verbatim body
//!   onto `app/{channel}`; minimal enforcement (non-empty name/channel). LOCAL/NORTHBOUND only.
//!
//! ## Accessors
//! Obtained from [`crate::EdgeCommonsInstance::data`]/[`crate::EdgeCommonsInstance::events`]/
//! [`crate::EdgeCommonsInstance::app`] (primary — the data plane is inherently per-instance) or the
//! `main`-instance convenience [`crate::EdgeCommons::data`]/[`crate::EdgeCommons::events`]/
//! [`crate::EdgeCommons::app`] (== `instance("main")`).
//!
//! ## Semantics & Architecture
//! - **Injected clock, no inline `Instant`/`SystemTime`**: every `serverTs`/`timestamp` default
//!   reads the bound [`Clock`] seam — production uses [`system_clock`] (the same "now" the
//!   envelope builder uses, `crate::messaging::message::now_rfc3339`); tests/vectors pin a fixed
//!   string, mirroring the `RepublishListener`'s injected-clock discipline
//!   (`crate::uns::RepublishListener`).
//! - **Reject vs default (§5.2)**: an omitted *defaultable* field (quality, serverTs, timestamp)
//!   is silently defaulted; an omitted *structural* field (`signal.id`, `evt.type`, `app`
//!   name/channel) is a fail-fast [`crate::EdgeCommonsError::Facade`] at the call site — never a dropped
//!   message.
//! - **Feature-gated stream route**: [`StreamSink`] is defined here (not in
//!   [`crate::streaming`]), so `DataFacade` compiles and behaves identically standalone; the
//!   `streaming` cargo feature only adds the *production* adapter
//!   (`crate::streaming::StreamServiceSink`) that lets [`crate::EdgeCommons`] wire a real sink. With
//!   the feature off (or no `streaming` config section), the bound sink is `None` and a
//!   `stream:<name>` channel falls back to a LOCAL publish (readiness / no-streaming → local, D1a).
//!
//! ## Related Modules
//! - [`crate::uns`] — the topic builder/validator + reserved-class guard these facades route
//!   through.
//! - [`crate::messaging`] — the guarded publish surface.
//! - [`crate::streaming`] — the telemetry-streaming service `DataFacade` composes (not replaces)
//!   for a `stream:<name>` channel.
//! - `docs/platform/DESIGN-class-facades.md` — the full design + decision register (D1-D9);
//!   `uns-test-vectors/{data,evt,app}.json` — the cross-language conformance vectors this module's
//!   `vector_tests` replay.

mod app;
mod channel;
mod data;
mod events;
mod quality;
mod severity;
mod signal_update;
mod stream_sink;

pub use app::{APP_MESSAGE_VERSION, AppCorrelation, AppFacade, PreparedAppMessage};
pub use channel::Channel;
pub use data::{DATA_MESSAGE_NAME, DATA_MESSAGE_VERSION, DataFacade, QUALITY_UNSPECIFIED};
pub use events::{EVT_MESSAGE_NAME, EVT_MESSAGE_VERSION, EventsFacade};
pub use quality::Quality;
pub use severity::Severity;
pub use signal_update::{Sample, SignalUpdate, SignalUpdateBuilder};
pub use stream_sink::StreamSink;

use std::sync::Arc;

/// The injected "now" seam for the `serverTs`/`timestamp` defaults (DESIGN-class-facades §2.1/
/// §2.2): an ISO-8601 UTC (`…Z`) string. Production binds [`system_clock`]; tests/vectors pin a
/// fixed closure — no facade body ever reads the wall clock inline, so every default is
/// deterministic under test.
pub type Clock = Arc<dyn Fn() -> String + Send + Sync>;

/// The production [`Clock`]: the system UTC "now" in RFC3339 (`…Z`) — the exact same "now" the
/// envelope builder stamps (`header.timestamp`), so a `data()`/`events()` publish and its
/// enclosing envelope agree on the instant.
pub fn system_clock() -> Clock {
    Arc::new(crate::messaging::message::now_rfc3339)
}

/// Cross-language conformance against `uns-test-vectors/{data,evt,app}.json`
/// (DESIGN-class-facades, mirroring the `crate::uns`/`crate::commands` vector-test pattern): every
/// case is replayed through a LIVE facade (a [`crate::testutil::RecordingMessaging`] + a recording
/// [`StreamSink`], a clock fixed at the vectors' pinned instant) and the resulting
/// `{topic, route, body[, partitionKey]}` (or `{throws: true}`) is asserted against the pinned
/// `expected`. `uns-test-vectors/envelopes.json`'s `data`/`evt`/`app` goldens are covered by the
/// existing class-agnostic `crate::uns::vector_tests::cross_language_envelope_vectors` test (it
/// rebuilds any class's envelope structurally + reproduces its topic byte-for-byte). Existence
/// -guarded: skipped when the vectors directory is absent.
#[cfg(test)]
mod vector_tests {
    use super::*;
    use crate::config::model::Config;
    use crate::messaging::{Message, MessagingService};
    use crate::testutil::RecordingMessaging;
    use crate::uns::{Uns, UnsClass};
    use serde_json::Value;
    use std::sync::Mutex;

    /// The vectors directory, or `None` (skip) when absent.
    fn vectors_dir() -> Option<std::path::PathBuf> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../uns-test-vectors");
        if dir.is_dir() {
            Some(dir)
        } else {
            eprintln!(
                "uns-test-vectors/ not found; skipping facades cross-language conformance vectors"
            );
            None
        }
    }

    fn load(dir: &std::path::Path, file: &str) -> Value {
        let bytes =
            std::fs::read(dir.join(file)).unwrap_or_else(|e| panic!("failed to read {file}: {e}"));
        serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("{file} is not valid JSON: {e}"))
    }

    /// The vectors' pinned clock: `2026-07-01T12:00:00Z`.
    fn fixed_clock() -> Clock {
        Arc::new(|| "2026-07-01T12:00:00Z".to_string())
    }

    /// The `gw-01`/`opcua-adapter` identity every facade vector is keyed to (instance `main` by
    /// default; `data.json` cases rebind to `kep1`).
    fn facade_config() -> Arc<Config> {
        Arc::new(Config::from_value("opcua-adapter", "gw-01", serde_json::json!({})).unwrap())
    }

    /// `(stream_name, partition_key, timestamp_ms, payload)` — one recorded stream append.
    type Recorded = (String, String, u64, Vec<u8>);

    /// A recording [`StreamSink`]: captures the last stream append.
    #[derive(Default)]
    struct RecordingStreamSink {
        last: Mutex<Option<Recorded>>,
    }

    impl StreamSink for RecordingStreamSink {
        fn append(
            &self,
            stream_name: &str,
            partition_key: &str,
            timestamp_ms: u64,
            payload: Vec<u8>,
        ) -> crate::error::Result<()> {
            *self.last.lock().unwrap() = Some((
                stream_name.to_string(),
                partition_key.to_string(),
                timestamp_ms,
                payload,
            ));
            Ok(())
        }
    }

    /// The single `(topic, route, body)` triple recorded on `messaging` — LOCAL or NORTHBOUND
    /// (mirrors the Java loader's `pm.qos == null ? "local" : "northbound"`).
    fn route_and_envelope(messaging: &RecordingMessaging) -> (String, &'static str, Value) {
        let local = messaging.local();
        if let Some((topic, msg)) = local.first() {
            return (topic.clone(), "local", msg.body.clone());
        }
        let iot = messaging.iot();
        let (topic, msg) = iot
            .first()
            .unwrap_or_else(|| panic!("no local or IoT Core publish recorded"));
        (topic.clone(), "northbound", msg.body.clone())
    }

    /// Runs one `data.json` case through a live [`DataFacade`]; returns `{topic, route, body[,
    /// partitionKey]}` or `{throws: true}` — the same shape `expected` pins.
    async fn run_data_case(input: &Value) -> Value {
        let instance_id = input
            .get("instance")
            .and_then(Value::as_str)
            .unwrap_or("kep1")
            .to_string();
        let config = facade_config();
        let messaging = RecordingMessaging::new();
        let sink = Arc::new(RecordingStreamSink::default());
        let identity = config
            .identity()
            .with_instance(instance_id.clone())
            .unwrap();
        let uns = Uns::new(identity, false);
        let facade = DataFacade::new(
            config.clone(),
            instance_id,
            uns.clone(),
            Some(messaging.clone() as Arc<dyn MessagingService>),
            Some(sink.clone() as Arc<dyn StreamSink>),
            fixed_clock(),
        );

        let mut builder = SignalUpdate::builder();
        if let Some(id) = input.get("signalId").and_then(Value::as_str) {
            builder = builder.signal_id(id);
        }
        if let Some(name) = input.get("signalName").and_then(Value::as_str) {
            builder = builder.name(name);
        }
        if let Some(address) = input.get("signalAddress").filter(|v| v.is_object()) {
            builder = builder.address(address.clone());
        }
        if let Some(device) = input.get("device").filter(|v| v.is_object()) {
            builder = builder.device(device.clone());
        }
        if let Some(path) = input.get("signalPath").and_then(Value::as_str) {
            builder = builder.signal_path(path);
        }
        if let Some(samples) = input.get("samples").and_then(Value::as_array) {
            for s in samples {
                let sample = Sample {
                    value: s.get("value").cloned(),
                    quality: s
                        .get("quality")
                        .and_then(Value::as_str)
                        .and_then(Quality::from_wire),
                    quality_raw: s
                        .get("qualityRaw")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    source_ts: s
                        .get("sourceTs")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    server_ts: s
                        .get("serverTs")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                };
                builder = builder.sample(sample);
            }
        }
        if let Some(over) = input.get("override").and_then(Value::as_str) {
            if let Some(ch) = Channel::from_config(over) {
                builder = builder.via(ch);
            }
        }

        match facade.publish(builder.build()).await {
            Err(_) => serde_json::json!({ "throws": true }),
            Ok(()) => {
                if !messaging.local().is_empty() || !messaging.iot().is_empty() {
                    let (topic, route, body) = route_and_envelope(&messaging);
                    serde_json::json!({ "topic": topic, "route": route, "body": body })
                } else {
                    // The stream route: re-derive the topic the same way the Java loader does
                    // (the StreamSink itself never receives the topic).
                    let path = input
                        .get("signalPath")
                        .and_then(Value::as_str)
                        .or_else(|| input.get("signalId").and_then(Value::as_str))
                        .expect("signalPath or signalId");
                    let topic = uns
                        .topic_with_channel(UnsClass::Data, &facade.channel_token(path).unwrap())
                        .unwrap();
                    let (stream_name, partition_key, _ts_millis, payload) = sink
                        .last
                        .lock()
                        .unwrap()
                        .clone()
                        .expect("a stream append was recorded");
                    let envelope = Message::from_slice(&payload).unwrap();
                    serde_json::json!({
                        "topic": topic,
                        "route": format!("stream:{stream_name}"),
                        "partitionKey": partition_key,
                        "body": envelope.body,
                    })
                }
            }
        }
    }

    /// Runs one `evt.json` case through a live [`EventsFacade`]; returns `{topic, route, body}`.
    async fn run_evt_case(input: &Value) -> Value {
        let config = facade_config();
        let messaging = RecordingMessaging::new();
        let uns = Uns::new(config.identity().clone(), false);
        let mut facade = EventsFacade::new(
            config,
            "main".to_string(),
            uns,
            Some(messaging.clone() as Arc<dyn MessagingService>),
            fixed_clock(),
        );
        if let Some(over) = input.get("override").and_then(Value::as_str) {
            if let Some(ch) = Channel::from_config(over) {
                facade = facade
                    .via(ch)
                    .expect("override is never a stream channel in evt.json");
            }
        }

        let kind = input["kind"].as_str().unwrap();
        let event_type = input["type"].as_str().unwrap().to_string();
        let message = input
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_string);
        let context = input.get("context").filter(|v| v.is_object()).cloned();
        let severity = input
            .get("severity")
            .and_then(Value::as_str)
            .and_then(Severity::from_wire);

        let result = match kind {
            "emit" => match severity {
                None => {
                    facade
                        .emit_message(event_type.clone(), message.clone().unwrap())
                        .await
                }
                Some(sev) => {
                    facade
                        .emit(sev, event_type.clone(), message.clone(), context.clone())
                        .await
                }
            },
            "raise" => match severity {
                None => {
                    facade
                        .raise_alarm_default(event_type.clone(), message.clone(), context.clone())
                        .await
                }
                Some(sev) => {
                    facade
                        .raise_alarm(sev, event_type.clone(), message.clone(), context.clone())
                        .await
                }
            },
            "clear" => match severity {
                None => {
                    facade
                        .clear_alarm_default(event_type.clone(), context.clone())
                        .await
                }
                Some(sev) => {
                    facade
                        .clear_alarm(sev, event_type.clone(), context.clone())
                        .await
                }
            },
            other => panic!("unknown evt kind '{other}'"),
        };
        result.unwrap_or_else(|e| panic!("evt case failed unexpectedly: {e}"));

        let (topic, route, body) = route_and_envelope(&messaging);
        serde_json::json!({ "topic": topic, "route": route, "body": body })
    }

    /// Runs one `app.json` case through a live [`AppFacade`]; returns `{topic, route, body}`.
    async fn run_app_case(input: &Value) -> Value {
        let config = facade_config();
        let messaging = RecordingMessaging::new();
        let uns = Uns::new(config.identity().clone(), false);
        let facade = AppFacade::new(
            config,
            "main".to_string(),
            uns,
            Some(messaging.clone() as Arc<dyn MessagingService>),
        );

        let name = input["name"].as_str().unwrap().to_string();
        let channel = input["channel"].as_str().unwrap().to_string();
        let body = input
            .get("body")
            .filter(|v| v.is_object())
            .cloned()
            .unwrap_or(serde_json::json!({}));
        let routing = input
            .get("override")
            .and_then(Value::as_str)
            .and_then(Channel::from_config);
        facade
            .publish_via(name, channel, body, routing)
            .await
            .unwrap_or_else(|e| panic!("app case failed unexpectedly: {e}"));

        let (topic, route, body) = route_and_envelope(&messaging);
        serde_json::json!({ "topic": topic, "route": route, "body": body })
    }

    #[tokio::test]
    async fn data_json_conformance() {
        let Some(dir) = vectors_dir() else { return };
        let doc = load(&dir, "data.json");
        let cases = doc["cases"].as_array().expect("cases");
        assert!(!cases.is_empty());
        for case in cases {
            let name = case["name"].as_str().unwrap();
            let got = run_data_case(&case["input"]).await;
            assert_eq!(&got, &case["expected"], "data case '{name}'");
        }
        eprintln!("uns-test-vectors data.json: {} cases OK", cases.len());
    }

    #[tokio::test]
    async fn evt_json_conformance() {
        let Some(dir) = vectors_dir() else { return };
        let doc = load(&dir, "evt.json");
        let cases = doc["cases"].as_array().expect("cases");
        assert!(!cases.is_empty());
        for case in cases {
            let name = case["name"].as_str().unwrap();
            let got = run_evt_case(&case["input"]).await;
            assert_eq!(&got, &case["expected"], "evt case '{name}'");
        }
        eprintln!("uns-test-vectors evt.json: {} cases OK", cases.len());
    }

    #[tokio::test]
    async fn app_json_conformance() {
        let Some(dir) = vectors_dir() else { return };
        let doc = load(&dir, "app.json");
        let cases = doc["cases"].as_array().expect("cases");
        assert!(!cases.is_empty());
        for case in cases {
            let name = case["name"].as_str().unwrap();
            let got = run_app_case(&case["input"]).await;
            assert_eq!(&got, &case["expected"], "app case '{name}'");
        }
        eprintln!("uns-test-vectors app.json: {} cases OK", cases.len());
    }

    /// Confirms `envelopes.json`'s `data`/`evt`/`app` goldens still exist and carry the real
    /// (non-stub) bodies these facades construct — the actual structural/topic conformance for
    /// them is asserted generically by
    /// `crate::uns::vector_tests::cross_language_envelope_vectors` (class-agnostic: rebuild +
    /// topic reproduction for every entry), so this just guards against the file drifting back to
    /// a stub shape without anyone noticing.
    #[test]
    fn envelopes_json_carries_the_real_facade_bodies() {
        let Some(dir) = vectors_dir() else { return };
        let doc = load(&dir, "envelopes.json");
        let envelopes = doc["envelopes"].as_array().expect("envelopes");
        let by_name = |name: &str| {
            envelopes
                .iter()
                .find(|e| e["name"] == name)
                .unwrap_or_else(|| panic!("envelopes.json missing '{name}'"))
        };

        let data = by_name("data-signal");
        assert_eq!(
            data["envelope"]["body"]["signal"]["id"],
            "ns=2;s=Line1.Temp"
        );
        assert_eq!(data["envelope"]["body"]["samples"][0]["quality"], "GOOD");
        assert_eq!(
            data["envelope"]["body"]["samples"][0]["qualityRaw"],
            "unspecified"
        );

        let evt = by_name("evt-info-door-open");
        assert_eq!(evt["envelope"]["body"]["severity"], "info");
        assert_eq!(evt["envelope"]["body"]["type"], "door-open");

        let app = by_name("app-hello");
        assert_eq!(app["envelope"]["header"]["name"], "OrderReceived");
        assert_eq!(app["envelope"]["body"]["greeting"], "hello");
    }
}
