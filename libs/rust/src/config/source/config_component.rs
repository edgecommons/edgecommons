//! # Configuration source — CONFIG_COMPONENT
//!
//! **One-liner purpose**: Load (and hot-reload) configuration from a dedicated
//! configuration-manager component over the UNS config rendezvous
//! (UNS-CANONICAL-DESIGN §4.3, D-U19 Flow A).
//!
//! ## Wire contract (a convention shared with the config server)
//! - **Flow A — GET**: a request to
//!   `ecv1/{device}/config/main/cmd/get-configuration` (with `{device}` = the
//!   sanitized resolved thing name). `config` is a **reserved-by-convention logical
//!   component name** — the config server is the sole subscriber and replies via
//!   `reply_to` with the configuration as the message body. Because this request
//!   runs during config bootstrap — *before* the [`Config`] snapshot (and therefore
//!   the component identity) exists — it carries **no envelope identity**; the
//!   requester **self-identifies in the body** with `{"component": "<short name>"}`
//!   (§1.5).
//! - **set-config push**: the server pushes a fire-and-forget `cmd` (no `reply_to`
//!   — a notification-style command) to the component's own inbox
//!   `ecv1/{device}/{component}/main/cmd/set-config`; the body is the new
//!   configuration, forwarded through [`ConfigSource::watch`] into the runtime's
//!   validate-and-swap reload path.
//!
//! The topics are minted locally from the resolved thing name and the component
//! name handed to the constructor (the same inputs the config layer later uses for
//! identity resolution) — never from a `Config`/`Uns`, which do not exist yet. Both
//! tokens pass the normative UNS token sanitizer
//! ([`crate::config::template::sanitize`]). These are `cmd`-class topics — not
//! library-reserved — so they ride the ordinary messaging surface (no
//! reserved-publish seam) and pass the reserved-topic guard.
//!
//! ## Semantics & Architecture
//! - `load` sends a `GetConfiguration` v1.0 request with an explicit 30 s per-call
//!   deadline (the §5 framework deadline — the reply subscription is cleaned up on
//!   expiry), retrying up to 3 attempts with a FRESH request each time (a settled
//!   future can never complete later).
//! - `watch` subscribes to the component's `set-config` inbox and forwards each
//!   message body. (Unlike the Java port's historical never-back-filled
//!   `parentConfigManager` bug, this path holds no config-manager reference at all —
//!   the runtime's reload task owns validation and the snapshot swap.)
//! - `Send + Sync`; async via `async_trait`. Errors map to
//!   [`crate::error::EdgeCommonsError::Config`].
//!
//! ## Related Modules
//! - [`super`], [`crate::messaging`].

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use super::ConfigSource;
use crate::config::identity::short_component_name;
use crate::config::template::sanitize;
use crate::error::{EdgeCommonsError, Result};
use crate::messaging::message::MessageBuilder;
use crate::messaging::{MessagingService, message_handler};

/// Flow-A GET request topic template (§4.3): the config server's rendezvous under
/// the reserved-by-convention logical component name `config`, instance `main`.
const GET_TOPIC_TEMPLATE: &str = "ecv1/{device}/config/main/cmd/get-configuration";
/// The pushed `set-config` command's topic template — this component's OWN inbox
/// (§4.3): the server-to-component push replacing the legacy `.../updated` topic.
const SET_CONFIG_TOPIC_TEMPLATE: &str = "ecv1/{device}/{component}/main/cmd/set-config";
/// Per-attempt reply deadline (the §5 framework deadline, passed per-call).
const REPLY_TIMEOUT: Duration = Duration::from_secs(30);
/// Maximum request attempts before giving up.
const MAX_ATTEMPTS: usize = 3;

/// Messaging-backed configuration-component source.
pub struct ConfigComponentSource {
    messaging: Arc<dyn MessagingService>,
    /// The sanitized short component name — the body self-identification token (§1.5).
    component_token: String,
    get_topic: String,
    set_config_topic: String,
}

/// Substitute the pre-sanitized `{device}` / `{component}` tokens into a topic
/// template. Deliberately local: the UNS builder is unavailable during config
/// bootstrap (§1.5), and these `cmd` topics need no reserved-class seam.
fn mint_topic(template: &str, device: &str, component: &str) -> String {
    template
        .replace("{device}", device)
        .replace("{component}", component)
}

impl ConfigComponentSource {
    /// Create a source bound to a messaging service and the component identity
    /// inputs (the resolved thing name + the full-or-short component name).
    pub fn new(
        messaging: Arc<dyn MessagingService>,
        thing_name: impl Into<String>,
        component_name: &str,
    ) -> Self {
        // Mint the UNS tokens locally (§1.5 steps 4-5): device = sanitized resolved
        // thing name, component = sanitized short name.
        let device_token = sanitize(&thing_name.into());
        let component_token = sanitize(short_component_name(component_name));
        Self {
            get_topic: mint_topic(GET_TOPIC_TEMPLATE, &device_token, &component_token),
            set_config_topic: mint_topic(
                SET_CONFIG_TOPIC_TEMPLATE,
                &device_token,
                &component_token,
            ),
            component_token,
            messaging,
        }
    }
}

#[async_trait]
impl ConfigSource for ConfigComponentSource {
    async fn load(&self) -> Result<Value> {
        let mut last_err = String::from("no attempts made");
        for attempt in 1..=MAX_ATTEMPTS {
            // The requester self-identifies in the BODY (§1.5): during bootstrap
            // there is no Config snapshot, so the envelope carries no identity
            // element — the config server routes on {"component"} instead.
            let request = MessageBuilder::new("GetConfiguration", "1.0")
                .payload(json!({ "component": self.component_token }))
                .build();
            // Explicit per-call §5 deadline: on expiry the supervisor has already
            // unsubscribed the ephemeral reply topic, so a retry must (and does)
            // issue a FRESH request.
            let reply_future = self
                .messaging
                .request_with_timeout(&self.get_topic, request, Some(REPLY_TIMEOUT))
                .await?;
            match reply_future.await {
                Ok(reply) => return Ok(reply.body),
                Err(EdgeCommonsError::RequestTimeout { .. }) => {
                    last_err = format!("timed out after {}s", REPLY_TIMEOUT.as_secs());
                    tracing::warn!(
                        attempt,
                        topic = %self.get_topic,
                        "config component request timed out; retrying"
                    );
                }
                Err(e) => last_err = e.to_string(),
            }
        }
        Err(EdgeCommonsError::Config(format!(
            "failed to load configuration from the config component after {MAX_ATTEMPTS} attempts: {last_err}"
        )))
    }

    fn source_name(&self) -> &str {
        "CONFIG_COMPONENT"
    }

    fn watch(&self) -> Option<mpsc::UnboundedReceiver<Value>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let messaging = self.messaging.clone();
        let topic = self.set_config_topic.clone();
        tokio::spawn(async move {
            let handler = message_handler(move |_topic, msg| {
                let tx = tx.clone();
                async move {
                    let _ = tx.send(msg.body);
                }
            });
            if let Err(e) = messaging.subscribe(&topic, handler, 16, 1).await {
                tracing::warn!(error = %e, topic = %topic, "failed to subscribe to the set-config inbox");
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

    /// A provider that (optionally) auto-replies to any request, records request
    /// payloads, and lets a test push messages into live subscriptions — enough to
    /// exercise request/reply + watch without a broker.
    struct FakeProvider {
        subs: Mutex<HashMap<String, SubSender>>,
        reply_body: Option<Value>,
        requests: Mutex<Vec<(String, Message)>>,
    }

    impl FakeProvider {
        fn new(reply_body: Option<Value>) -> Arc<Self> {
            Arc::new(Self {
                subs: Mutex::new(HashMap::new()),
                reply_body,
                requests: Mutex::new(Vec::new()),
            })
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
        async fn publish(&self, t: &str, payload: Vec<u8>, _d: Destination, _q: Qos) -> Result<()> {
            if let Ok(req) = Message::from_slice(&payload) {
                self.requests
                    .lock()
                    .unwrap()
                    .push((t.to_string(), req.clone()));
                if let Some(body) = &self.reply_body {
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
        fn connected(&self) -> bool {
            true
        }
    }

    #[test]
    fn topics_are_the_uns_rendezvous_and_own_inbox() {
        assert_eq!(
            mint_topic(GET_TOPIC_TEMPLATE, "gw-01", "MyComp"),
            "ecv1/gw-01/config/main/cmd/get-configuration"
        );
        assert_eq!(
            mint_topic(SET_CONFIG_TOPIC_TEMPLATE, "gw-01", "MyComp"),
            "ecv1/gw-01/MyComp/main/cmd/set-config"
        );
    }

    #[tokio::test]
    async fn load_fetches_config_and_self_identifies_in_the_body() {
        let provider = FakeProvider::new(Some(serde_json::json!({ "feature": "on", "n": 5 })));
        let svc: Arc<dyn MessagingService> =
            Arc::new(DefaultMessagingService::new(provider.clone()));
        let src = ConfigComponentSource::new(svc, "thing-1", "com.example.MyComp");

        let doc = src.load().await.unwrap();
        assert_eq!(doc["feature"], "on");
        assert_eq!(doc["n"], 5);
        assert_eq!(src.source_name(), "CONFIG_COMPONENT");

        // The Flow-A request went to the config server's rendezvous, with the
        // requester self-identified in the BODY and NO envelope identity (§1.5).
        let requests = provider.requests.lock().unwrap();
        let (topic, request) = &requests[0];
        assert_eq!(topic, "ecv1/thing-1/config/main/cmd/get-configuration");
        assert_eq!(
            request.body["component"], "MyComp",
            "sanitized short name in the body"
        );
        assert!(
            request.identity.is_none(),
            "bootstrap request carries no identity"
        );
    }

    #[tokio::test]
    async fn tokens_are_sanitized_into_the_topics() {
        let provider = FakeProvider::new(Some(serde_json::json!({})));
        let svc: Arc<dyn MessagingService> =
            Arc::new(DefaultMessagingService::new(provider.clone()));
        let src = ConfigComponentSource::new(svc, "thing+1", "com.example.My/Comp");
        let _ = src.load().await.unwrap();
        let requests = provider.requests.lock().unwrap();
        assert_eq!(
            requests[0].0,
            "ecv1/thing_1/config/main/cmd/get-configuration"
        );
        assert_eq!(requests[0].1.body["component"], "My_Comp");
    }

    #[tokio::test]
    async fn load_errors_when_request_fails() {
        // RecordingMessaging.request returns Err -> load propagates it.
        let svc: Arc<dyn MessagingService> = crate::testutil::RecordingMessaging::new();
        let src = ConfigComponentSource::new(svc, "t", "c");
        assert!(src.load().await.is_err());
    }

    #[tokio::test]
    async fn watch_forwards_set_config_pushes_from_the_own_inbox() {
        let provider = FakeProvider::new(None);
        let svc: Arc<dyn MessagingService> =
            Arc::new(DefaultMessagingService::new(provider.clone()));
        let src = ConfigComponentSource::new(svc, "thing-1", "com.example.MyComp");

        let mut rx = src.watch().unwrap();
        let inbox = "ecv1/thing-1/MyComp/main/cmd/set-config";
        for _ in 0..100 {
            if provider.has_sub(inbox) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            provider.has_sub(inbox),
            "watch should subscribe to the set-config inbox"
        );

        let update = crate::messaging::message::MessageBuilder::new("cmd", "1.0")
            .payload(serde_json::json!({ "v": 9 }))
            .build();
        provider.push(inbox, &update);

        let body = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("update delivered within timeout")
            .expect("a body");
        assert_eq!(body["v"], 9);
    }
}
