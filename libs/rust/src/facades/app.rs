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
use std::time::Duration;

use serde_json::Value;

use crate::config::model::Config;
use crate::config::template::sanitize;
use crate::error::{EdgeCommonsError, Result};
use crate::messaging::message::{Message, MessageBuilder};
use crate::messaging::{MessagingService, Qos};
use crate::uns::{Uns, UnsClass};

use super::Channel;

/// The app envelope header version (the header `name` is the caller's chosen name).
pub const APP_MESSAGE_VERSION: &str = "1.0";

/// A correlation source for [`AppFacade::prepare_correlated`].
///
/// Construct it implicitly from a received [`Message`], `&str`, or [`String`]. A request supplies
/// its standard envelope correlation id; no request body or reply path is retained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppCorrelation(String);

impl From<&Message> for AppCorrelation {
    fn from(request: &Message) -> Self {
        Self(request.header.correlation_id.clone())
    }
}

impl From<&str> for AppCorrelation {
    fn from(correlation_id: &str) -> Self {
        Self(correlation_id.to_string())
    }
}

impl From<String> for AppCorrelation {
    fn from(correlation_id: String) -> Self {
        Self(correlation_id)
    }
}

/// One immutable application message ready for ordinary or confirmed publication.
///
/// `encoded` is produced exactly once from `message`. Confirmed prepared publication sends these
/// bytes verbatim, which lets a durable outbox persist and retry one stable envelope UUID without
/// rebuilding or reserializing it.
#[derive(Debug, Clone, PartialEq)]
pub struct PreparedAppMessage {
    topic: String,
    message: Message,
    encoded: Vec<u8>,
}

impl PreparedAppMessage {
    /// The final `app/{channel}` topic.
    pub fn topic(&self) -> &str {
        &self.topic
    }

    /// The identity- and correlation-stamped envelope.
    pub fn message(&self) -> &Message {
        &self.message
    }

    /// The exact protobuf envelope bytes used by confirmed publication.
    pub fn encoded(&self) -> &[u8] {
        &self.encoded
    }
}

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
        AppFacade {
            config,
            instance_id,
            uns,
            messaging,
        }
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
        let prepared = self.prepare(name, channel, body)?;
        self.publish_prepared(&prepared).await
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
        let prepared = self.prepare(name, channel, body)?;
        self.publish_prepared_via(&prepared, routing).await
    }

    /// Build and serialize one local application message without publishing it.
    ///
    /// The returned [`PreparedAppMessage`] owns the exact bytes used by confirmed publication.
    pub fn prepare(
        &self,
        name: impl Into<String>,
        channel: impl Into<String>,
        body: Value,
    ) -> Result<PreparedAppMessage> {
        self.prepare_inner(name.into(), channel.into(), body, None)
    }

    /// Build and serialize one application message with an explicit correlation source.
    ///
    /// `correlation` accepts either a received request (`&Message`) or an explicit correlation id
    /// (`&str`/`String`). The supplied value is stamped on the standard envelope header.
    pub fn prepare_correlated(
        &self,
        name: impl Into<String>,
        channel: impl Into<String>,
        body: Value,
        correlation: impl Into<AppCorrelation>,
    ) -> Result<PreparedAppMessage> {
        let correlation = correlation.into().0;
        if correlation.is_empty() {
            return Err(EdgeCommonsError::Facade(
                "app correlation id must be non-empty".to_string(),
            ));
        }
        self.prepare_inner(name.into(), channel.into(), body, Some(correlation))
    }

    fn prepare_inner(
        &self,
        name: String,
        channel: String,
        body: Value,
        correlation_id: Option<String>,
    ) -> Result<PreparedAppMessage> {
        if name.is_empty() {
            return Err(EdgeCommonsError::Facade(
                "app publish requires a non-empty header name".to_string(),
            ));
        }
        if channel.is_empty() {
            return Err(EdgeCommonsError::Facade(
                "app publish requires a non-empty channel".to_string(),
            ));
        }
        let token = channel
            .split('/')
            .map(sanitize)
            .collect::<Vec<_>>()
            .join("/");
        let topic = self.uns.topic_with_channel(UnsClass::App, &token)?;
        let mut builder = MessageBuilder::new(name, APP_MESSAGE_VERSION)
            .from_config(&self.config)
            .instance(self.instance_id.clone())
            .payload(body);
        if let Some(correlation_id) = correlation_id {
            builder = builder.correlation_id(correlation_id);
        }
        let message = builder.build();
        let encoded = message.to_vec()?;
        Ok(PreparedAppMessage {
            topic,
            message,
            encoded,
        })
    }

    /// Publish a prepared message locally using the legacy enqueue-confirmed publish path.
    pub async fn publish_prepared(&self, prepared: &PreparedAppMessage) -> Result<()> {
        self.publish_prepared_via(prepared, None).await
    }

    /// Publish a prepared message using legacy LOCAL/NORTHBOUND routing semantics.
    ///
    /// Northbound failures remain logged and swallowed for compatibility with [`Self::publish_via`].
    pub async fn publish_prepared_via(
        &self,
        prepared: &PreparedAppMessage,
        routing: Option<Channel>,
    ) -> Result<()> {
        if routing.as_ref().is_some_and(Channel::is_stream) {
            return Err(EdgeCommonsError::Facade(
                "app() does not support the stream channel - use data() for streamed telemetry"
                    .to_string(),
            ));
        }
        if matches!(routing, Some(Channel::Northbound)) {
            let messaging = self.messaging()?;
            if let Err(e) = messaging
                .publish_northbound(&prepared.topic, &prepared.message, Qos::AtLeastOnce)
                .await
            {
                tracing::warn!(
                    topic = prepared.topic,
                    error = %e,
                    "northbound app publish failed (local readiness unaffected)"
                );
            }
            Ok(())
        } else {
            self.messaging()?
                .publish(&prepared.topic, &prepared.message)
                .await
        }
    }

    /// Publish prepared bytes locally and await strict QoS 1 transport confirmation.
    pub async fn publish_prepared_confirmed(
        &self,
        prepared: &PreparedAppMessage,
        timeout: Duration,
    ) -> Result<()> {
        self.publish_prepared_confirmed_via(prepared, None, timeout)
            .await
    }

    /// Confirmed LOCAL/NORTHBOUND publication of one prepared envelope.
    ///
    /// Unlike legacy northbound routing, confirmation failures are returned to the caller. The
    /// stored bytes are sent verbatim and never regenerated from [`PreparedAppMessage::message`].
    pub async fn publish_prepared_confirmed_via(
        &self,
        prepared: &PreparedAppMessage,
        routing: Option<Channel>,
        timeout: Duration,
    ) -> Result<()> {
        if routing.as_ref().is_some_and(Channel::is_stream) {
            return Err(EdgeCommonsError::Facade(
                "app() does not support the stream channel - use data() for streamed telemetry"
                    .to_string(),
            ));
        }
        if matches!(routing, Some(Channel::Northbound)) {
            self.messaging()?
                .publish_northbound_encoded_confirmed(&prepared.topic, &prepared.encoded, timeout)
                .await
        } else {
            self.messaging()?
                .publish_encoded_confirmed(&prepared.topic, &prepared.encoded, timeout)
                .await
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
        AppFacade::new(
            config,
            "main".to_string(),
            uns,
            Some(messaging as Arc<dyn MessagingService>),
        )
    }

    #[tokio::test]
    async fn publish_passes_the_body_verbatim_with_the_named_header() {
        let messaging = RecordingMessaging::new();
        let f = facade(messaging.clone());
        f.publish(
            "OrderReceived",
            "order/received",
            json!({ "orderId": "A-42", "qty": 3 }),
        )
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
        assert_eq!(
            messaging.local()[0].0,
            "ecv1/gw-01/opcua-adapter/main/app/a_b"
        );
    }

    #[tokio::test]
    async fn empty_name_or_channel_is_rejected() {
        let messaging = RecordingMessaging::new();
        let f = facade(messaging);
        assert!(matches!(
            f.publish("", "c", json!({})).await,
            Err(EdgeCommonsError::Facade(_))
        ));
        assert!(matches!(
            f.publish("N", "", json!({})).await,
            Err(EdgeCommonsError::Facade(_))
        ));
    }

    #[tokio::test]
    async fn stream_routing_is_rejected() {
        let messaging = RecordingMessaging::new();
        let f = facade(messaging);
        assert!(matches!(
            f.publish_via("N", "c", json!({}), Some(Channel::stream("hot").unwrap()))
                .await,
            Err(EdgeCommonsError::Facade(_))
        ));
    }

    #[tokio::test]
    async fn northbound_routing_publishes_to_northbound() {
        let messaging = RecordingMessaging::new();
        let f = facade(messaging.clone());
        f.publish_via(
            "CloudEvent",
            "cloud",
            json!({ "k": "v" }),
            Some(Channel::Northbound),
        )
        .await
        .unwrap();
        assert!(messaging.local().is_empty());
        assert_eq!(messaging.iot().len(), 1);
    }

    #[test]
    fn prepare_correlated_accepts_request_or_explicit_id() {
        let messaging = RecordingMessaging::new();
        let f = facade(messaging);
        let request = MessageBuilder::new("sb/capture", "1.0")
            .correlation_id("request-correlation")
            .command(json!({}))
            .build();

        let from_request = f
            .prepare_correlated(
                "ImageCaptured",
                "image/captured",
                json!({ "captureId": "cap-1" }),
                &request,
            )
            .unwrap();
        let from_id = f
            .prepare_correlated(
                "ImageCaptured",
                "image/captured",
                json!({ "captureId": "cap-2" }),
                "explicit-correlation",
            )
            .unwrap();

        assert_eq!(
            from_request.message().header.correlation_id,
            "request-correlation"
        );
        assert_eq!(
            from_id.message().header.correlation_id,
            "explicit-correlation"
        );
        assert_eq!(
            from_request.encoded(),
            from_request.message().to_vec().unwrap()
        );
    }

    #[test]
    fn prepare_correlated_rejects_an_empty_correlation_id() {
        let messaging = RecordingMessaging::new();
        let f = facade(messaging);

        let error = f
            .prepare_correlated("ImageCaptured", "image/captured", json!({}), "")
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("correlation id must be non-empty")
        );
    }

    #[tokio::test]
    async fn confirmed_prepared_publish_sends_the_stored_bytes_without_reserialization() {
        let messaging = RecordingMessaging::new();
        let f = facade(messaging.clone());
        let mut prepared = f
            .prepare_correlated(
                "ImageCaptured",
                "image/captured",
                json!({ "captureId": "cap-1" }),
                "corr-1",
            )
            .unwrap();
        let expected = prepared.encoded().to_vec();

        // A durable outbox owns the encoded envelope. Even if another in-memory Message copy is
        // later changed, confirmed publication must send the persisted bytes, not reserialize it.
        prepared.message.header.name = "MutatedAfterPrepare".to_string();
        f.publish_prepared_confirmed(&prepared, Duration::from_secs(1))
            .await
            .unwrap();

        let confirmed = messaging.confirmed_local();
        assert_eq!(confirmed.len(), 1);
        assert_eq!(confirmed[0].0, prepared.topic());
        assert_eq!(confirmed[0].1, expected);
        assert_eq!(
            Message::from_slice(&confirmed[0].1).unwrap().header.name,
            "ImageCaptured"
        );
    }

    #[tokio::test]
    async fn confirmed_northbound_failure_is_returned_not_swallowed() {
        let messaging = RecordingMessaging::new();
        messaging.fail_next_confirmed(1);
        let f = facade(messaging.clone());
        let prepared = f.prepare("CloudEvent", "cloud", json!({})).unwrap();

        assert!(
            f.publish_prepared_confirmed_via(
                &prepared,
                Some(Channel::Northbound),
                Duration::from_secs(1),
            )
            .await
            .is_err()
        );
        assert!(messaging.confirmed_iot().is_empty());
    }
}
