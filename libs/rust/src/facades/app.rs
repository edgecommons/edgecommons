//! # AppFacade — the `app()` publish facade
//!
//! **One-liner purpose**: Free-form inter-component pub/sub on the `app` class
//! (DESIGN-class-facades §2.3, D3), mirroring the Java canonical
//! `com.mbreissi.edgecommons.facades.AppFacade`.
//!
//! `app` is the intentionally-open class — the facade's value is **not** body enforcement (there
//! is no contract to enforce), it is removing the raw three-line ritual and guaranteeing topic +
//! identity correctness: a **named** header, the developer body **verbatim**, minted onto
//! `app/{channel}` with the envelope identity stamped. `app` is non-reserved — this publishes
//! through the ordinary guarded `messaging().publish(...)`.
//!
//! **Routing:** [`Channel::Local`] (default) or [`Channel::Northbound`]; a stream route is
//! **rejected** (same reasoning as `events()`).

use std::sync::Arc;

use serde_json::Value;

use crate::config::model::Config;
use crate::config::template::sanitize;
use crate::error::{EdgeCommonsError, Result};
use crate::messaging::message::MessageBuilder;
use crate::messaging::{MessagingService, Qos};
use crate::uns::{Uns, UnsClass};

use super::Channel;

/// The app envelope header version (the header `name` is the caller's chosen name).
pub const APP_MESSAGE_VERSION: &str = "1.0";

/// The `app()` publish facade bound to one instance — see the [module docs](self). Obtain via
/// [`crate::EdgeCommonsInstance::app`] (or the `main`-instance convenience [`crate::EdgeCommons::app`]).
pub struct AppFacade {
    config: Arc<Config>,
    instance_id: String,
    uns: Uns,
    messaging: Option<Arc<dyn MessagingService>>,
}

impl AppFacade {
    /// Library-internal constructor (see the [`crate::EdgeCommonsInstance::app`] wiring).
    pub(crate) fn new(
        config: Arc<Config>,
        instance_id: String,
        uns: Uns,
        messaging: Option<Arc<dyn MessagingService>>,
    ) -> AppFacade {
        AppFacade { config, instance_id, uns, messaging }
    }

    /// Publishes a free-form message on `app/{channel}` locally.
    ///
    /// - `name` — the envelope header `name` (the developer's message name; REQUIRED).
    /// - `channel` — the `app/{channel}` tail (each `/`-token is sanitized; REQUIRED).
    /// - `body` — the developer body, published verbatim.
    pub async fn publish(
        &self,
        name: impl Into<String>,
        channel: impl Into<String>,
        body: Value,
    ) -> Result<()> {
        self.publish_via(name, channel, body, None).await
    }

    /// [`Self::publish`] with an explicit LOCAL/NORTHBOUND routing.
    ///
    /// # Errors
    /// [`EdgeCommonsError::Facade`] when `name`/`channel` is empty, or `routing` is a stream channel.
    pub async fn publish_via(
        &self,
        name: impl Into<String>,
        channel: impl Into<String>,
        body: Value,
        routing: Option<Channel>,
    ) -> Result<()> {
        let name = name.into();
        let channel = channel.into();
        if name.is_empty() {
            return Err(EdgeCommonsError::Facade("app publish requires a non-empty header name".to_string()));
        }
        if channel.is_empty() {
            return Err(EdgeCommonsError::Facade("app publish requires a non-empty channel".to_string()));
        }
        if routing.as_ref().is_some_and(Channel::is_stream) {
            return Err(EdgeCommonsError::Facade(
                "app() does not support the stream channel - use data() for streamed telemetry"
                    .to_string(),
            ));
        }
        let token = channel.split('/').map(sanitize).collect::<Vec<_>>().join("/");
        let topic = self.uns.topic_with_channel(UnsClass::App, &token)?;
        let msg = MessageBuilder::new(name, APP_MESSAGE_VERSION)
            .from_config(&self.config)
            .instance(self.instance_id.clone())
            .payload(body)
            .build();
        if matches!(routing, Some(Channel::Northbound)) {
            let messaging = self.messaging()?;
            if let Err(e) = messaging.publish_to_iot_core(&topic, &msg, Qos::AtLeastOnce).await {
                tracing::warn!(
                    topic,
                    error = %e,
                    "northbound app publish failed (local readiness unaffected)"
                );
            }
            Ok(())
        } else {
            self.messaging()?.publish(&topic, &msg).await
        }
    }

    fn messaging(&self) -> Result<&Arc<dyn MessagingService>> {
        self.messaging.as_ref().ok_or_else(|| {
            EdgeCommonsError::Messaging(
                "messaging is not available: app() requires a wired messaging transport"
                    .to_string(),
            )
        })
    }

    /// The instance token this facade is bound to.
    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::RecordingMessaging;
    use serde_json::json;

    fn facade(messaging: Arc<RecordingMessaging>) -> AppFacade {
        let config = Arc::new(Config::from_value("opcua-adapter", "gw-01", json!({})).unwrap());
        let uns = Uns::new(config.identity().clone(), false);
        AppFacade::new(config, "main".to_string(), uns, Some(messaging as Arc<dyn MessagingService>))
    }

    #[tokio::test]
    async fn publish_passes_the_body_verbatim_with_the_named_header() {
        let messaging = RecordingMessaging::new();
        let f = facade(messaging.clone());
        f.publish("OrderReceived", "order/received", json!({ "orderId": "A-42", "qty": 3 }))
            .await
            .unwrap();
        let (topic, msg) = &messaging.local()[0];
        assert_eq!(topic, "ecv1/gw-01/opcua-adapter/main/app/order/received");
        assert_eq!(msg.header.name, "OrderReceived");
        assert_eq!(msg.body, json!({ "orderId": "A-42", "qty": 3 }));
    }

    #[tokio::test]
    async fn channel_is_sanitized() {
        let messaging = RecordingMessaging::new();
        let f = facade(messaging.clone());
        f.publish("Ping", "a+b", json!({ "n": 1 })).await.unwrap();
        assert_eq!(messaging.local()[0].0, "ecv1/gw-01/opcua-adapter/main/app/a_b");
    }

    #[tokio::test]
    async fn empty_name_or_channel_is_rejected() {
        let messaging = RecordingMessaging::new();
        let f = facade(messaging);
        assert!(matches!(f.publish("", "c", json!({})).await, Err(EdgeCommonsError::Facade(_))));
        assert!(matches!(f.publish("N", "", json!({})).await, Err(EdgeCommonsError::Facade(_))));
    }

    #[tokio::test]
    async fn stream_routing_is_rejected() {
        let messaging = RecordingMessaging::new();
        let f = facade(messaging);
        assert!(matches!(
            f.publish_via("N", "c", json!({}), Some(Channel::stream("hot").unwrap())).await,
            Err(EdgeCommonsError::Facade(_))
        ));
    }

    #[tokio::test]
    async fn northbound_routing_publishes_to_iot_core() {
        let messaging = RecordingMessaging::new();
        let f = facade(messaging.clone());
        f.publish_via("CloudEvent", "cloud", json!({ "k": "v" }), Some(Channel::Northbound))
            .await
            .unwrap();
        assert!(messaging.local().is_empty());
        assert_eq!(messaging.iot().len(), 1);
    }
}
