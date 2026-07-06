//! # Configuration — effective-config (`cfg`) publisher
//!
//! **One-liner purpose**: The library-owned `cfg` publisher (UNS-CANONICAL-DESIGN
//! §4.3): announces the component's effective (redacted) configuration on
//! `ecv1/{device}/{component}/main/cfg` — once at startup (after initialization
//! completes) and again on every configuration change.
//!
//! The body is `{"config": <effective config, redacted>}`; the `cfg` class is
//! reserved, so the publish goes through the privileged [`ReservedMessaging`] seam.
//! (This is the push half only — the `republish-cfg` pull verb lands in a later
//! phase.)
//!
//! **Redaction v1** (§4.3): `$secret` references are never resolved (the raw config
//! is published as-is, so a `{"$secret": …}` ref stays a ref); every value under a
//! `credentials` key inside the top-level `messaging` section, and every value of a
//! key named `password` or `pin` (case-insensitive) anywhere, is replaced with
//! `"***"`.
//!
//! ## Related Modules
//! - [`super::model`], [`crate::messaging`], [`crate::uns`].

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::config::ConfigurationChangeListener;
use crate::config::model::Config;
use crate::messaging::ReservedMessaging;
use crate::messaging::message::MessageBuilder;
use crate::uns::{Uns, UnsClass};

/// The cfg announcement's envelope header name (§4.3).
const CFG_MESSAGE_NAME: &str = "cfg";
const CFG_MESSAGE_VERSION: &str = "1.0";
/// The redaction placeholder.
const REDACTED: &str = "***";

/// Publishes the effective (redacted) configuration on the component's UNS `cfg`
/// topic — at startup ([`Self::publish_now`], called by the runtime builder) and on
/// every hot reload (registered as a [`ConfigurationChangeListener`]).
pub(crate) struct EffectiveConfigPublisher {
    reserved: Arc<dyn ReservedMessaging>,
}

impl EffectiveConfigPublisher {
    /// Creates the publisher over the privileged reserved-publish seam (§4.2).
    pub(crate) fn new(reserved: Arc<dyn ReservedMessaging>) -> Self {
        Self { reserved }
    }

    /// Publishes the effective (redacted) configuration to the component's UNS
    /// `cfg` topic. Best-effort: any failure is logged and swallowed — a cfg
    /// announcement must never crash the component.
    pub(crate) async fn publish_now(&self, config: &Config) {
        // The RAW includeRoot flag (Java parity): Uns applies D-U25 internally.
        let uns = Uns::new(config.identity().clone(), config.topic_include_root());
        let topic = match uns.topic(UnsClass::Cfg) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(error = %e, "effective-config publish failed to build its UNS topic");
                return;
            }
        };
        let body = json!({ "config": redact(&config.raw) });
        let message = MessageBuilder::new(CFG_MESSAGE_NAME, CFG_MESSAGE_VERSION)
            .payload(body)
            .from_config(config)
            .build();
        match self.reserved.publish_reserved(&topic, &message).await {
            Ok(()) => {
                tracing::debug!(topic = %topic, "published effective (redacted) configuration")
            }
            Err(e) => tracing::warn!(error = %e, topic = %topic, "effective-config publish failed"),
        }
    }
}

#[async_trait]
impl ConfigurationChangeListener for EffectiveConfigPublisher {
    /// Each successful hot reload republishes the effective config.
    async fn on_configuration_change(&self, config: Arc<Config>) -> bool {
        self.publish_now(&config).await;
        true
    }
}

/// Redaction v1 (§4.3) over a deep copy of the effective config: every value of a
/// key named `password` or `pin` (case-insensitive, anywhere) and every value of a
/// `credentials` key at any depth inside the **top-level** `messaging` section
/// becomes the string [`REDACTED`]. `$secret` refs are untouched (they are never
/// resolved here, so no secret material exists to leak).
pub(crate) fn redact(config: &Value) -> Value {
    let mut copy = config.clone();
    redact_value(&mut copy, false, true);
    copy
}

/// Recursive redaction walk. `in_messaging` is true anywhere under the top-level
/// `messaging` section (the `messaging.*.credentials` rule); `top_level` is true
/// only for the config root, so a nested `messaging` key elsewhere does not
/// trigger the credentials rule.
fn redact_value(value: &mut Value, in_messaging: bool, top_level: bool) {
    match value {
        Value::Object(map) => {
            for (key, entry) in map.iter_mut() {
                let lower = key.to_ascii_lowercase();
                if lower == "password" || lower == "pin" || (in_messaging && lower == "credentials")
                {
                    *entry = Value::String(REDACTED.to_string());
                    continue;
                }
                redact_value(
                    entry,
                    in_messaging || (top_level && key == "messaging"),
                    false,
                );
            }
        }
        Value::Array(items) => {
            for item in items {
                if item.is_object() {
                    redact_value(item, in_messaging, false);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::RecordingMessaging;
    use serde_json::json;

    #[test]
    fn redacts_password_and_pin_anywhere_case_insensitively() {
        let raw = json!({
            "component": { "global": { "db": { "Password": "hunter2", "PIN": "1234" } } },
            "list": [ { "password": "x" }, 42 ]
        });
        let redacted = redact(&raw);
        assert_eq!(redacted["component"]["global"]["db"]["Password"], "***");
        assert_eq!(redacted["component"]["global"]["db"]["PIN"], "***");
        assert_eq!(redacted["list"][0]["password"], "***");
        assert_eq!(redacted["list"][1], 42);
    }

    #[test]
    fn redacts_credentials_only_under_top_level_messaging() {
        let raw = json!({
            "messaging": {
                "local": { "credentials": { "username": "u", "certPath": "c" } },
                "northbound": { "credentials": { "keyPath": "k" } }
            },
            "component": { "global": { "credentials": { "token": "keep-me" } } }
        });
        let redacted = redact(&raw);
        assert_eq!(redacted["messaging"]["local"]["credentials"], "***");
        assert_eq!(redacted["messaging"]["northbound"]["credentials"], "***");
        assert_eq!(
            redacted["component"]["global"]["credentials"]["token"], "keep-me",
            "a credentials key OUTSIDE the top-level messaging section is untouched"
        );
    }

    #[test]
    fn secret_refs_stay_unresolved_refs() {
        let raw = json!({ "streaming": { "apiKey": { "$secret": "kafka/sasl" } } });
        let redacted = redact(&raw);
        assert_eq!(redacted["streaming"]["apiKey"]["$secret"], "kafka/sasl");
    }

    #[test]
    fn nested_messaging_key_elsewhere_does_not_trigger_the_credentials_rule() {
        let raw = json!({
            "component": { "global": { "messaging": { "credentials": "keep" } } }
        });
        let redacted = redact(&raw);
        assert_eq!(
            redacted["component"]["global"]["messaging"]["credentials"],
            "keep"
        );
    }

    #[test]
    fn redaction_does_not_mutate_the_source() {
        let raw = json!({ "messaging": { "local": { "credentials": { "password": "p" } } } });
        let _ = redact(&raw);
        assert_eq!(raw["messaging"]["local"]["credentials"]["password"], "p");
    }

    #[tokio::test]
    async fn publishes_the_redacted_config_on_the_cfg_topic_via_the_seam() {
        let config = Config::from_value(
            "com.example.MyComp",
            "thing-1",
            json!({
                "messaging": { "local": { "host": "h", "port": 1883, "clientId": "c",
                                          "credentials": { "username": "u", "password": "p" } } },
                "component": { "global": { "publish_interval": 3 } }
            }),
        )
        .unwrap();
        let recorder = RecordingMessaging::new();
        let publisher = EffectiveConfigPublisher::new(recorder.clone());

        publisher.publish_now(&config).await;

        let published = recorder.reserved_local();
        assert_eq!(published.len(), 1, "one cfg announcement through the seam");
        let (topic, msg) = &published[0];
        assert_eq!(topic, "ecv1/thing-1/MyComp/main/cfg");
        assert_eq!(msg.header.name, "cfg");
        assert_eq!(msg.header.version, "1.0");
        assert_eq!(
            msg.body["config"]["messaging"]["local"]["credentials"],
            "***"
        );
        assert_eq!(
            msg.body["config"]["component"]["global"]["publish_interval"],
            3
        );
        let identity = msg
            .identity
            .as_ref()
            .expect("cfg envelope carries identity");
        assert_eq!(identity.component(), "MyComp");
        assert!(
            recorder.local().is_empty(),
            "must use the SEAM, not publish()"
        );
    }

    #[tokio::test]
    async fn republishes_on_configuration_change() {
        let config =
            Arc::new(Config::from_value("com.example.MyComp", "thing-1", json!({})).unwrap());
        let recorder = RecordingMessaging::new();
        let publisher = EffectiveConfigPublisher::new(recorder.clone());
        assert!(publisher.on_configuration_change(config).await);
        assert_eq!(recorder.reserved_local().len(), 1);
    }
}
