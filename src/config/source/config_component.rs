//! # Configuration source — CONFIG_COMPONENT (Phase 2)
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
