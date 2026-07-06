//! # EdgeCommonsInstance — the per-instance seam
//!
//! **One-liner purpose**: The instance-scoped handle (UNS-CANONICAL-DESIGN §3,
//! D-U3) whose only job is to pre-bind the instance token into (a) the [`Uns`]
//! topic builder, (b) the [`MessageBuilder`], and (c) the app-usable publish
//! facades (`data()`/`events()`/`app()` — DESIGN-class-facades §3).
//!
//! The messaging service stays instance-agnostic — `publish(topic, msg)` already
//! receives both the topic (minted by this handle's instance-bound [`Uns`]) and the
//! envelope (stamped by its instance-bound builder). Component-level messages
//! (everything not built through a handle) default to instance `"main"`.
//!
//! Obtain handles from [`crate::EdgeCommons::instance`] (token validated against the
//! §2.2 rule). The id is deliberately NOT verified against the configured
//! `component.instances[]` — instances may be created dynamically; an unknown id is
//! only logged at DEBUG as a diagnostic aid.
//!
//! ## Usage Example
//! ```no_run
//! # async fn demo(gg: &edgecommons::EdgeCommons) -> edgecommons::Result<()> {
//! use edgecommons::uns::UnsClass;
//! let kep1 = gg.instance("kep1")?;
//! let topic = kep1.uns().topic_with_channel(UnsClass::Data, "temp")?;
//! let msg = kep1.message("data", "1.0").payload(serde_json::json!({ "v": 1 })).build();
//! gg.messaging()?.publish(&topic, &msg).await?;
//!
//! // The data()/events()/app() publish facades (DESIGN-class-facades), bound to "kep1":
//! kep1.data().publish_value("temp", 21.5).await?;
//! # Ok(())
//! # }
//! ```

use std::sync::Arc;

use crate::config::model::Config;
use crate::error::Result;
use crate::facades::{AppFacade, Clock, DataFacade, EventsFacade, StreamSink};
use crate::messaging::MessagingService;
use crate::messaging::message::MessageBuilder;
use crate::uns::Uns;

/// The per-instance seam (UNS-CANONICAL-DESIGN §3, D-U3): an instance-scoped
/// handle over a configuration snapshot. See the [module docs](self).
pub struct EdgeCommonsInstance {
    id: String,
    config: Arc<Config>,
    uns: Uns,
    /// `None` only when no messaging transport was wired; the `data()`/`events()`/`app()`
    /// facades then fail their publish calls with [`crate::EdgeCommonsError::Messaging`] instead of
    /// silently dropping (mirrors [`crate::EdgeCommons::messaging`]'s own `Result`-returning
    /// accessor).
    messaging: Option<Arc<dyn MessagingService>>,
    /// The `data()` facade's stream-route seam (DESIGN-class-facades §4); `None` when
    /// streaming is not configured (or the `streaming` cargo feature is off) — a `stream:`
    /// channel then falls back to a LOCAL publish (D1a).
    stream_sink: Option<Arc<dyn StreamSink>>,
    /// The injected "now" seam the `data()`/`events()` facades use for their `serverTs`/
    /// `timestamp` defaults (no inline `Instant`/`SystemTime` call in a facade body).
    clock: Clock,
}

impl EdgeCommonsInstance {
    /// Crate-private: created by [`crate::EdgeCommons::instance`], which validates
    /// the token (§2.2 token rule) first.
    pub(crate) fn new(
        id: String,
        config: Arc<Config>,
        messaging: Option<Arc<dyn MessagingService>>,
        stream_sink: Option<Arc<dyn StreamSink>>,
        clock: Clock,
    ) -> Result<EdgeCommonsInstance> {
        let identity = config.identity().with_instance(id.clone())?;
        // The RAW includeRoot flag, like gg.uns(): Uns applies it per-target only
        // for multi-level hierarchies (D-U25).
        let uns = Uns::new(identity, config.topic_include_root());
        Ok(EdgeCommonsInstance {
            id,
            config,
            uns,
            messaging,
            stream_sink,
            clock,
        })
    }

    /// Returns this handle's instance token.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the topic builder bound to this instance (topics minted with this
    /// instance token).
    pub fn uns(&self) -> &Uns {
        &self.uns
    }

    /// Starts a message pre-bound to this instance — equivalent to
    /// `MessageBuilder::new(name, version).from_config(&config).instance(id())`, so
    /// `build()` stamps the component identity with this handle's instance token.
    pub fn message(&self, name: impl Into<String>, version: impl Into<String>) -> MessageBuilder {
        MessageBuilder::new(name, version)
            .from_config(&self.config)
            .instance(self.id.clone())
    }

    /// The `data()` publish facade bound to this instance (DESIGN-class-facades §2.1): builds +
    /// validates the `SouthboundSignalUpdate` body (quality → GOOD, `serverTs` → now, samples
    /// wrapper), sanitizes the signal path into the `data` channel, and routes on the resolved
    /// channel (per-call ▸ config `publish.channel` ▸ LOCAL).
    pub fn data(&self) -> DataFacade {
        DataFacade::new(
            self.config.clone(),
            self.id.clone(),
            self.uns.clone(),
            self.messaging.clone(),
            self.stream_sink.clone(),
            self.clock.clone(),
        )
    }

    /// The `events()` publish facade bound to this instance (DESIGN-class-facades §2.2):
    /// operator events & alarms on the `evt` class, deriving the `evt/{severity}/{type}` channel
    /// from the body.
    pub fn events(&self) -> EventsFacade {
        EventsFacade::new(
            self.config.clone(),
            self.id.clone(),
            self.uns.clone(),
            self.messaging.clone(),
            self.clock.clone(),
        )
    }

    /// The `app()` publish facade bound to this instance (DESIGN-class-facades §2.3): free-form
    /// inter-component pub/sub on the `app` class (named header + verbatim body).
    pub fn app(&self) -> AppFacade {
        AppFacade::new(
            self.config.clone(),
            self.id.clone(),
            self.uns.clone(),
            self.messaging.clone(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::RecordingMessaging;
    use crate::uns::UnsClass;
    use serde_json::json;

    fn config() -> Arc<Config> {
        Arc::new(Config::from_value("com.example.MyComp", "gw-01", json!({})).unwrap())
    }

    fn test_clock() -> Clock {
        Arc::new(|| "2026-07-01T12:00:00Z".to_string())
    }

    fn handle(id: &str, messaging: Option<Arc<dyn MessagingService>>) -> EdgeCommonsInstance {
        EdgeCommonsInstance::new(id.to_string(), config(), messaging, None, test_clock()).unwrap()
    }

    #[test]
    fn handle_binds_the_instance_into_uns_and_messages() {
        let handle = handle("kep1", None);
        assert_eq!(handle.id(), "kep1");
        assert_eq!(
            handle
                .uns()
                .topic_with_channel(UnsClass::Data, "temp")
                .unwrap(),
            "ecv1/gw-01/MyComp/kep1/data/temp"
        );
        let msg = handle
            .message("data", "1.0")
            .payload(json!({ "v": 1 }))
            .build();
        assert_eq!(msg.identity.unwrap().instance(), "kep1");
        assert_eq!(msg.header.name, "data");
    }

    #[tokio::test]
    async fn data_events_app_facades_are_bound_to_this_instance() {
        let messaging = RecordingMessaging::new();
        let handle = handle("kep1", Some(messaging.clone() as Arc<dyn MessagingService>));

        handle.data().publish_value("temp", 21.5).await.unwrap();
        handle
            .events()
            .emit_message("door-open", "front door opened")
            .await
            .unwrap();
        handle
            .app()
            .publish("Hello", "hello", json!({ "greeting": "hi" }))
            .await
            .unwrap();

        let published = messaging.local();
        assert_eq!(published.len(), 3);
        assert_eq!(published[0].0, "ecv1/gw-01/MyComp/kep1/data/temp");
        assert_eq!(published[1].0, "ecv1/gw-01/MyComp/kep1/evt/info/door-open");
        assert_eq!(published[2].0, "ecv1/gw-01/MyComp/kep1/app/hello");
    }

    #[tokio::test]
    async fn facades_error_when_no_messaging_is_wired() {
        let handle = handle("kep1", None);
        assert!(matches!(
            handle.data().publish_value("temp", 1).await,
            Err(crate::EdgeCommonsError::Messaging(_))
        ));
    }
}
