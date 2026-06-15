//! # Messaging — service layer
//!
//! **One-liner purpose**: The transport-agnostic [`MessagingService`] (publish /
//! subscribe / request / reply over [`Message`]s) and its default implementation
//! over any [`MessagingProvider`].
//!
//! ## Overview
//! [`DefaultMessagingService`] wraps an `Arc<dyn MessagingProvider>` and adds
//! message (de)serialization and request/reply correlation. It is built once and
//! works unchanged over MQTT (standalone) or IPC (Greengrass).
//!
//! ## Semantics & Architecture
//! - Async (`tokio`); object-safe via `async_trait`.
//! - Request/reply uses a per-request ephemeral reply topic
//!   ([`crate::messaging::request_reply`]); the reply subscription is dropped when
//!   the request returns or times out, so nothing leaks.
//! - Error handling: [`crate::error::Result`]; timeouts and closed channels are
//!   reported as [`GgError::Messaging`], never panics.
//!
//! ## Usage Example
//! ```no_run
//! # async fn demo(provider: std::sync::Arc<dyn ggcommons::messaging::MessagingProvider>) -> ggcommons::Result<()> {
//! use ggcommons::messaging::service::{DefaultMessagingService, MessagingService};
//! use ggcommons::messaging::{message::MessageBuilder, Destination};
//! use std::time::Duration;
//!
//! let svc = DefaultMessagingService::new(provider);
//! let req = MessageBuilder::new("Ping", "1.0").thing_name("t").build();
//! let _reply = svc.request("svc/ping", req, Destination::Local, Duration::from_secs(5)).await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Design Choices
//! - Replies are published on [`Destination::Local`]; cross-destination reply
//!   routing is a later refinement (standalone request/reply runs on the local bus).
//!
//! ## Related Modules
//! - [`crate::messaging::provider`], [`crate::messaging::message`].

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use super::message::Message;
use super::{request_reply, Destination, MessageStream, MessagingProvider, Qos};
use crate::error::{GgError, Result};

/// Transport-agnostic messaging operations over [`Message`]s.
#[async_trait]
pub trait MessagingService: Send + Sync {
    /// Publish a message to `topic` on `dest`.
    async fn publish(&self, topic: &str, msg: &Message, dest: Destination) -> Result<()>;

    /// Subscribe to `filter` on `dest`, yielding deserialized messages.
    async fn subscribe(&self, filter: &str, dest: Destination) -> Result<MessageStream>;

    /// Send `msg` to `topic` and await a single correlated reply, up to `timeout`.
    async fn request(
        &self,
        topic: &str,
        msg: Message,
        dest: Destination,
        timeout: Duration,
    ) -> Result<Message>;

    /// Reply to a previously received request message.
    async fn reply(&self, request: &Message, reply: Message) -> Result<()>;
}

/// Default [`MessagingService`] built over a [`MessagingProvider`].
pub struct DefaultMessagingService {
    provider: Arc<dyn MessagingProvider>,
}

impl DefaultMessagingService {
    /// Wrap a provider in the default service.
    pub fn new(provider: Arc<dyn MessagingProvider>) -> Self {
        Self { provider }
    }
}

#[async_trait]
impl MessagingService for DefaultMessagingService {
    async fn publish(&self, topic: &str, msg: &Message, dest: Destination) -> Result<()> {
        let bytes = msg.to_vec()?;
        self.provider.publish(topic, bytes, dest, Qos::AtLeastOnce).await
    }

    async fn subscribe(&self, filter: &str, dest: Destination) -> Result<MessageStream> {
        let sub = self.provider.subscribe(filter, dest, Qos::AtLeastOnce).await?;
        Ok(MessageStream::new(sub))
    }

    /// Send a request and await its reply.
    ///
    /// # Algorithmic Choices
    /// Subscribes to a unique reply topic, stamps it as the request's `replyTo`,
    /// publishes, then awaits the first message with [`tokio::time::timeout`]. The
    /// reply subscription is dropped on return (success, timeout, or error),
    /// guaranteeing cleanup.
    ///
    /// # Errors
    /// | Error Variant | Condition | Recovery |
    /// |---------------|-----------|----------|
    /// | `GgError::Messaging` | Timed out, channel closed, or transport failure | Retry; check the responder |
    /// | `GgError::Json` | Reply payload was not a valid message | Validate the responder's format |
    async fn request(
        &self,
        topic: &str,
        msg: Message,
        dest: Destination,
        timeout: Duration,
    ) -> Result<Message> {
        let reply_topic = request_reply::new_reply_topic();
        let mut sub = self.provider.subscribe(&reply_topic, dest, Qos::AtLeastOnce).await?;

        let mut request = msg;
        request.header.reply_to = Some(reply_topic);
        self.provider
            .publish(topic, request.to_vec()?, dest, Qos::AtLeastOnce)
            .await?;

        match tokio::time::timeout(timeout, sub.recv()).await {
            Ok(Some((_topic, bytes))) => Message::from_slice(&bytes),
            Ok(None) => Err(GgError::Messaging(
                "reply channel closed before a reply arrived".to_string(),
            )),
            Err(_) => Err(GgError::Messaging("request timed out".to_string())),
        }
    }

    /// Publish a reply correlated with `request`.
    ///
    /// # Pre-conditions
    /// `request.header.reply_to` is set (i.e. it was created via `request`).
    ///
    /// # Errors
    /// | Error Variant | Condition | Recovery |
    /// |---------------|-----------|----------|
    /// | `GgError::Messaging` | The request carried no `replyTo`, or publish failed | Ensure the inbound message is a request |
    async fn reply(&self, request: &Message, reply: Message) -> Result<()> {
        let topic = request.header.reply_to.clone().ok_or_else(|| {
            GgError::Messaging("cannot reply: request has no replyTo".to_string())
        })?;

        let mut reply = reply;
        reply.header.correlation_id = request.header.correlation_id.clone();
        self.provider
            .publish(&topic, reply.to_vec()?, Destination::Local, Qos::AtLeastOnce)
            .await
    }
}
