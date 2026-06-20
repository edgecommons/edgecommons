//! # Configuration source — CONFIG_COMPONENT
//!
//! **One-liner purpose**: Load (and hot-reload) configuration from a dedicated
//! configuration-manager component via request/reply over messaging.
//!
//! ## Overview
//! Transport-agnostic: it works over whichever [`crate::messaging::MessagingService`]
//! the runtime wired (Greengrass IPC in GREENGRASS mode, dual-broker MQTT in
//! STANDALONE mode). The topic contract matches the Java/Python libraries verbatim
//! (cross-language parity):
//! - request: `ggcommons/{ThingName}/config/get/{ComponentName}`
//! - updated: `ggcommons/{ThingName}/config/{ComponentName}/updated`
//!
//! ## Semantics & Architecture
//! - `load` sends a `GetConfiguration` v1.0 request and awaits the reply (30s
//!   timeout, up to 3 attempts), returning the reply body as the config document.
//! - `watch` subscribes to the updated topic and forwards each message body.
//! - `Send + Sync`; async via `async_trait`. Errors map to
//!   [`crate::error::GgError::Config`].
//!
//! ## Status
//! Implemented over the shared messaging service. The underlying request/reply
//! mechanism is validated on a live Greengrass core (via the local request/reply
//! path); a standalone configuration-manager component was not separately deployed.
//!
//! ## Related Modules
//! - [`super`], [`crate::messaging`].

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::mpsc;

use super::ConfigSource;
use crate::error::{GgError, Result};
use crate::messaging::message::MessageBuilder;
use crate::messaging::{message_handler, MessagingService};

/// Request-topic template (parity with Java/Python).
const GET_TOPIC_TEMPLATE: &str = "ggcommons/{ThingName}/config/get/{ComponentName}";
/// Updated-topic template (parity with Java/Python).
const UPDATED_TOPIC_TEMPLATE: &str = "ggcommons/{ThingName}/config/{ComponentName}/updated";
/// Per-attempt reply timeout.
const REPLY_TIMEOUT: Duration = Duration::from_secs(30);
/// Maximum request attempts before giving up.
const MAX_ATTEMPTS: usize = 3;

/// Messaging-backed configuration-component source.
pub struct ConfigComponentSource {
    messaging: Arc<dyn MessagingService>,
    thing_name: String,
    get_topic: String,
    updated_topic: String,
}

/// Substitute `{ThingName}` / `{ComponentName}` into a topic template.
fn resolve_topic(template: &str, thing: &str, component: &str) -> String {
    template
        .replace("{ThingName}", thing)
        .replace("{ComponentName}", component)
}

impl ConfigComponentSource {
    /// Create a source bound to a messaging service and the component identity.
    pub fn new(
        messaging: Arc<dyn MessagingService>,
        thing_name: impl Into<String>,
        component_name: &str,
    ) -> Self {
        let thing_name = thing_name.into();
        Self {
            get_topic: resolve_topic(GET_TOPIC_TEMPLATE, &thing_name, component_name),
            updated_topic: resolve_topic(UPDATED_TOPIC_TEMPLATE, &thing_name, component_name),
            thing_name,
            messaging,
        }
    }
}

#[async_trait]
impl ConfigSource for ConfigComponentSource {
    async fn load(&self) -> Result<Value> {
        let mut last_err = String::from("no attempts made");
        for attempt in 1..=MAX_ATTEMPTS {
            let request = MessageBuilder::new("GetConfiguration", "1.0")
                .thing_name(&self.thing_name)
                .payload(json!({}))
                .build();
            let reply_future = self.messaging.request(&self.get_topic, request).await?;
            match tokio::time::timeout(REPLY_TIMEOUT, reply_future).await {
                Ok(Ok(reply)) => return Ok(reply.body),
                Ok(Err(e)) => last_err = e.to_string(),
                Err(_) => {
                    last_err = format!("timed out after {}s", REPLY_TIMEOUT.as_secs());
                    tracing::warn!(attempt, topic = %self.get_topic, "config component request timed out; retrying");
                }
            }
        }
        Err(GgError::Config(format!(
            "failed to load configuration from the config component after {MAX_ATTEMPTS} attempts: {last_err}"
        )))
    }

    fn source_name(&self) -> &str {
        "CONFIG_COMPONENT"
    }

    fn watch(&self) -> Option<mpsc::UnboundedReceiver<Value>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let messaging = self.messaging.clone();
        let topic = self.updated_topic.clone();
        tokio::spawn(async move {
            let handler = message_handler(move |_topic, msg| {
                let tx = tx.clone();
                async move {
                    let _ = tx.send(msg.body);
                }
            });
            if let Err(e) = messaging.subscribe(&topic, handler, 16, 1).await {
                tracing::warn!(error = %e, topic = %topic, "failed to subscribe to config-updated topic");
            }
        });
        Some(rx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::time::Duration;

    use crate::messaging::message::Message;
    use crate::messaging::service::DefaultMessagingService;
    use crate::messaging::{Destination, MessagingProvider, Qos, Subscription};

    /// Bounded delivery channel for one fake subscription.
    type SubSender = tokio::sync::mpsc::Sender<(String, Vec<u8>)>;

    /// A provider that (optionally) auto-replies to any request, and lets a test push
    /// messages into live subscriptions — enough to exercise request/reply + watch
    /// without a broker.
    struct FakeProvider {
        subs: Mutex<HashMap<String, SubSender>>,
        reply_body: Option<Value>,
    }

    impl FakeProvider {
        fn new(reply_body: Option<Value>) -> Arc<Self> {
            Arc::new(Self { subs: Mutex::new(HashMap::new()), reply_body })
        }
        fn has_sub(&self, topic: &str) -> bool {
            self.subs.lock().unwrap().contains_key(topic)
        }
        fn push(&self, topic: &str, msg: &Message) {
            if let Some(tx) = self.subs.lock().unwrap().get(topic) {
                let _ = tx.try_send((topic.to_string(), msg.to_vec().unwrap()));
            }
        }
    }

    #[async_trait]
    impl MessagingProvider for FakeProvider {
        async fn publish(&self, _t: &str, payload: Vec<u8>, _d: Destination, _q: Qos) -> Result<()> {
            if let Some(body) = &self.reply_body {
                if let Ok(req) = Message::from_slice(&payload) {
                    if let Some(reply_to) = req.header.reply_to.clone() {
                        let reply = crate::messaging::message::MessageBuilder::new("Config", "1.0")
                            .correlation_id(req.header.correlation_id.clone())
                            .payload(body.clone())
                            .build();
                        if let Some(tx) = self.subs.lock().unwrap().get(&reply_to) {
                            let _ = tx.try_send((reply_to.clone(), reply.to_vec().unwrap()));
                        }
                    }
                }
            }
            Ok(())
        }
        async fn subscribe(
            &self,
            filter: &str,
            _d: Destination,
            _q: Qos,
            max: usize,
        ) -> Result<Subscription> {
            let (tx, rx) = tokio::sync::mpsc::channel(max.max(1));
            self.subs.lock().unwrap().insert(filter.to_string(), tx);
            Ok(Subscription::new(rx, Box::new(())))
        }
        async fn unsubscribe(&self, filter: &str, _d: Destination) -> Result<()> {
            self.subs.lock().unwrap().remove(filter);
            Ok(())
        }
    }

    #[test]
    fn topic_templates_resolve() {
        assert_eq!(
            resolve_topic(GET_TOPIC_TEMPLATE, "T", "C"),
            "ggcommons/T/config/get/C"
        );
        assert_eq!(
            resolve_topic(UPDATED_TOPIC_TEMPLATE, "T", "C"),
            "ggcommons/T/config/C/updated"
        );
    }

    #[tokio::test]
    async fn load_fetches_config_via_request_reply() {
        let provider = FakeProvider::new(Some(serde_json::json!({ "feature": "on", "n": 5 })));
        let svc: Arc<dyn MessagingService> = Arc::new(DefaultMessagingService::new(provider));
        let src = ConfigComponentSource::new(svc, "thing-1", "com.example.C");

        let doc = src.load().await.unwrap();
        assert_eq!(doc["feature"], "on");
        assert_eq!(doc["n"], 5);
        assert_eq!(src.source_name(), "CONFIG_COMPONENT");
    }

    #[tokio::test]
    async fn load_errors_when_request_fails() {
        // RecordingMessaging.request returns Err -> load propagates it.
        let svc: Arc<dyn MessagingService> = crate::testutil::RecordingMessaging::new();
        let src = ConfigComponentSource::new(svc, "t", "c");
        assert!(src.load().await.is_err());
    }

    #[tokio::test]
    async fn watch_forwards_updated_config_messages() {
        let provider = FakeProvider::new(None);
        let svc: Arc<dyn MessagingService> = Arc::new(DefaultMessagingService::new(provider.clone()));
        let src = ConfigComponentSource::new(svc, "thing-1", "com.example.C");

        let mut rx = src.watch().unwrap();
        let updated = "ggcommons/thing-1/config/com.example.C/updated";
        for _ in 0..100 {
            if provider.has_sub(updated) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(provider.has_sub(updated), "watch should subscribe to the updated topic");

        let update = crate::messaging::message::MessageBuilder::new("ConfigUpdated", "1.0")
            .payload(serde_json::json!({ "v": 9 }))
            .build();
        provider.push(updated, &update);

        let body = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("update delivered within timeout")
            .expect("a body");
        assert_eq!(body["v"], 9);
    }
}
