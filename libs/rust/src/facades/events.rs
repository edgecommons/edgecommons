//! # EventsFacade — the `events()` publish facade
//!
//! **One-liner purpose**: Operator events & alarms on the `evt` class (DESIGN-class-facades §2.2,
//! D8), mirroring the Java canonical `com.mbreissi.edgecommons.facades.EventsFacade`.
//!
//! This is the facade that stops the historical `evt` drift (DESIGN-class-facades §1.2): it makes
//! the `evt/{severity}/{type}` channel and the body shape non-negotiable by **deriving the channel
//! from the body's own `severity` + `type`**, so the topic and body can never disagree. `evt` is
//! non-reserved — this publishes through the ordinary guarded `messaging().publish(...)`.
//!
//! **Routing:** [`Channel::Local`] (default) or [`Channel::Northbound`] via [`EventsFacade::via`]
//! — alarms often go straight to the cloud control plane. A stream route is **rejected** (events
//! are low-rate control-plane, not bulk telemetry).

use std::sync::Arc;

use serde_json::{Map, Value};

use crate::config::model::Config;
use crate::config::template::sanitize;
use crate::error::{EdgeCommonsError, Result};
use crate::messaging::message::MessageBuilder;
use crate::messaging::{MessagingService, Qos};
use crate::uns::{Uns, UnsClass};

use super::{Channel, Clock, Severity};

/// The event envelope header name.
pub const EVT_MESSAGE_NAME: &str = "evt";
/// The event envelope header version.
pub const EVT_MESSAGE_VERSION: &str = "1.0";

/// The `events()` publish facade bound to one instance — see the [module docs](self). Obtain via
/// [`crate::EdgeCommonsInstance::events`] (or the `main`-instance convenience
/// [`crate::EdgeCommons::events`]).
#[derive(Clone)]
pub struct EventsFacade {
    config: Arc<Config>,
    instance_id: String,
    uns: Uns,
    messaging: Option<Arc<dyn MessagingService>>,
    clock: Clock,
    /// The per-view routing override (`None` = LOCAL default); set by [`Self::via`].
    via: Option<Channel>,
}

impl EventsFacade {
    /// Library-internal constructor (see the [`crate::EdgeCommonsInstance::events`] wiring).
    pub(crate) fn new(
        config: Arc<Config>,
        instance_id: String,
        uns: Uns,
        messaging: Option<Arc<dyn MessagingService>>,
        clock: Clock,
    ) -> EventsFacade {
        EventsFacade { config, instance_id, uns, messaging, clock, via: None }
    }

    /// Returns a channel-bound view for a per-call routing override (LOCAL or NORTHBOUND).
    ///
    /// # Errors
    /// [`EdgeCommonsError::Facade`] when `channel` is a stream channel.
    pub fn via(&self, channel: Channel) -> Result<EventsFacade> {
        if channel.is_stream() {
            return Err(EdgeCommonsError::Facade(
                "events() does not support the stream channel - events are low-rate \
                 control-plane, not bulk telemetry (use data() for streamed telemetry)"
                    .to_string(),
            ));
        }
        let mut bound = self.clone();
        bound.via = Some(channel);
        Ok(bound)
    }

    // ===================== emit =====================

    /// Emits a one-shot event with an explicit severity, optional message, and optional
    /// structured context.
    pub async fn emit(
        &self,
        severity: Severity,
        event_type: impl Into<String>,
        message: Option<String>,
        context: Option<Value>,
    ) -> Result<()> {
        self.publish(severity, event_type.into(), message, context, None).await
    }

    /// Message-only convenience — severity defaults to [`Severity::Info`].
    pub async fn emit_message(
        &self,
        event_type: impl Into<String>,
        message: impl Into<String>,
    ) -> Result<()> {
        self.emit(Severity::Info, event_type, Some(message.into()), None).await
    }

    // ===================== alarms =====================

    /// Raises a stateful alarm (`alarm=true, active=true`) with an explicit severity.
    pub async fn raise_alarm(
        &self,
        severity: Severity,
        event_type: impl Into<String>,
        message: Option<String>,
        context: Option<Value>,
    ) -> Result<()> {
        self.publish(severity, event_type.into(), message, context, Some(true)).await
    }

    /// Raises a stateful alarm — severity defaults to [`Severity::Critical`] so raises and clears
    /// of the same alarm ride the same `evt/critical/{type}` channel (subsumes OPC UA's
    /// `connection-lost`).
    pub async fn raise_alarm_default(
        &self,
        event_type: impl Into<String>,
        message: Option<String>,
        context: Option<Value>,
    ) -> Result<()> {
        self.raise_alarm(Severity::Critical, event_type, message, context).await
    }

    /// Clears a stateful alarm (`alarm=true, active=false`) with an explicit severity (must match
    /// the raise's severity to land on the same channel).
    pub async fn clear_alarm(
        &self,
        severity: Severity,
        event_type: impl Into<String>,
        context: Option<Value>,
    ) -> Result<()> {
        self.publish(severity, event_type.into(), None, context, Some(false)).await
    }

    /// Clears a stateful alarm — severity defaults to [`Severity::Critical`] so the clear tracks
    /// on the same channel as the raise (subsumes OPC UA's `connection-restored`).
    pub async fn clear_alarm_default(
        &self,
        event_type: impl Into<String>,
        context: Option<Value>,
    ) -> Result<()> {
        self.clear_alarm(Severity::Critical, event_type, context).await
    }

    // ===================== body construction + routing =====================

    /// Constructs the `evt` wire body — the exact body the vectors pin. Deterministic given the
    /// injected clock. Member order: severity, type, message?, timestamp, context?, alarm?,
    /// active?.
    ///
    /// - `active` — `Some(active)` for a `raiseAlarm`/`clearAlarm` (sets `alarm=true,
    ///   active=<active>`); `None` for a plain [`Self::emit`] (no `alarm`/`active` fields).
    ///
    /// # Errors
    /// [`EdgeCommonsError::Facade`] when `event_type` is empty.
    pub fn build_body(
        &self,
        severity: Severity,
        event_type: &str,
        message: Option<&str>,
        context: Option<&Value>,
        active: Option<bool>,
    ) -> Result<Value> {
        if event_type.is_empty() {
            return Err(EdgeCommonsError::Facade(
                "evt requires a non-empty type (it is a channel token and the event's kind)"
                    .to_string(),
            ));
        }
        let mut body = Map::new();
        body.insert("severity".to_string(), Value::String(severity.wire().to_string()));
        body.insert("type".to_string(), Value::String(event_type.to_string()));
        if let Some(message) = message {
            body.insert("message".to_string(), Value::String(message.to_string()));
        }
        body.insert("timestamp".to_string(), Value::String((self.clock)()));
        if let Some(context) = context {
            body.insert("context".to_string(), context.clone());
        }
        if let Some(active) = active {
            body.insert("alarm".to_string(), Value::Bool(true));
            body.insert("active".to_string(), Value::Bool(active));
        }
        Ok(Value::Object(body))
    }

    /// The `evt/{severity}/{type}` channel derived from the body's own severity + type.
    ///
    /// # Errors
    /// [`EdgeCommonsError::Facade`] when `event_type` is empty.
    pub fn channel_for(severity: Severity, event_type: &str) -> Result<String> {
        if event_type.is_empty() {
            return Err(EdgeCommonsError::Facade("evt requires a non-empty type".to_string()));
        }
        Ok(format!("{}/{}", severity.wire(), sanitize(event_type)))
    }

    async fn publish(
        &self,
        severity: Severity,
        event_type: String,
        message: Option<String>,
        context: Option<Value>,
        active: Option<bool>,
    ) -> Result<()> {
        let body =
            self.build_body(severity, &event_type, message.as_deref(), context.as_ref(), active)?;
        let channel = Self::channel_for(severity, &event_type)?;
        let topic = self.uns.topic_with_channel(UnsClass::Evt, &channel)?;
        let msg = MessageBuilder::new(EVT_MESSAGE_NAME, EVT_MESSAGE_VERSION)
            .from_config(&self.config)
            .instance(self.instance_id.clone())
            .payload(body)
            .build();
        self.route(&topic, msg).await
    }

    /// LOCAL (default) or NORTHBOUND; a stream override is rejected up front by [`Self::via`].
    async fn route(&self, topic: &str, msg: crate::messaging::message::Message) -> Result<()> {
        if matches!(self.via, Some(Channel::Northbound)) {
            let messaging = self.messaging()?;
            if let Err(e) = messaging.publish_to_iot_core(topic, &msg, Qos::AtLeastOnce).await {
                tracing::warn!(
                    topic,
                    error = %e,
                    "northbound evt publish failed (local readiness unaffected)"
                );
            }
            Ok(())
        } else {
            self.messaging()?.publish(topic, &msg).await
        }
    }

    fn messaging(&self) -> Result<&Arc<dyn MessagingService>> {
        self.messaging.as_ref().ok_or_else(|| {
            EdgeCommonsError::Messaging(
                "messaging is not available: events() requires a wired messaging transport"
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

    fn fixed_clock() -> Clock {
        Arc::new(|| "2026-07-01T12:00:00Z".to_string())
    }

    fn facade(messaging: Arc<RecordingMessaging>) -> EventsFacade {
        let config = Arc::new(Config::from_value("opcua-adapter", "gw-01", json!({})).unwrap());
        let uns = Uns::new(config.identity().clone(), false);
        EventsFacade::new(
            config,
            "main".to_string(),
            uns,
            Some(messaging as Arc<dyn MessagingService>),
            fixed_clock(),
        )
    }

    #[tokio::test]
    async fn emit_message_only_defaults_to_info() {
        let messaging = RecordingMessaging::new();
        let f = facade(messaging.clone());
        f.emit_message("door-open", "front door opened").await.unwrap();
        let (topic, msg) = &messaging.local()[0];
        assert_eq!(topic, "ecv1/gw-01/opcua-adapter/main/evt/info/door-open");
        assert_eq!(msg.body["severity"], "info");
        assert_eq!(msg.body["type"], "door-open");
        assert_eq!(msg.body["message"], "front door opened");
        assert_eq!(msg.body["timestamp"], "2026-07-01T12:00:00Z");
        assert!(msg.body.get("alarm").is_none());
    }

    #[tokio::test]
    async fn raise_and_clear_alarm_default_to_critical_and_share_a_channel() {
        let messaging = RecordingMessaging::new();
        let f = facade(messaging.clone());
        f.raise_alarm_default("connection-lost", Some("link down".to_string()), None)
            .await
            .unwrap();
        f.clear_alarm_default("connection-lost", None).await.unwrap();
        let published = messaging.local();
        assert_eq!(published.len(), 2);
        assert_eq!(published[0].0, "ecv1/gw-01/opcua-adapter/main/evt/critical/connection-lost");
        assert_eq!(published[1].0, published[0].0, "raise and clear share the same channel");
        assert_eq!(published[0].1.body["alarm"], true);
        assert_eq!(published[0].1.body["active"], true);
        assert_eq!(published[1].1.body["active"], false);
    }

    #[tokio::test]
    async fn channel_type_is_sanitized() {
        let messaging = RecordingMessaging::new();
        let f = facade(messaging.clone());
        f.emit(Severity::Info, "a+b", Some("x".to_string()), None).await.unwrap();
        assert_eq!(messaging.local()[0].0, "ecv1/gw-01/opcua-adapter/main/evt/info/a_b");
    }

    #[tokio::test]
    async fn empty_type_is_rejected() {
        let messaging = RecordingMessaging::new();
        let f = facade(messaging);
        assert!(matches!(
            f.emit(Severity::Info, "", None, None).await,
            Err(EdgeCommonsError::Facade(_))
        ));
    }

    #[tokio::test]
    async fn via_northbound_routes_to_iot_core_and_rejects_stream() {
        let messaging = RecordingMessaging::new();
        let f = facade(messaging.clone());
        assert!(f.via(Channel::stream("hot").unwrap()).is_err());
        let north = f.via(Channel::Northbound).unwrap();
        north.emit(Severity::Critical, "overtemp", None, None).await.unwrap();
        assert!(messaging.local().is_empty());
        assert_eq!(messaging.iot().len(), 1);
    }
}
