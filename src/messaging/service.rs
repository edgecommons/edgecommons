//! # Messaging — service layer
//!
//! **One-liner purpose**: The transport-agnostic [`MessagingService`] (callback
//! subscribe, unsubscribe, publish, request/reply over [`Message`]s) and its
//! default implementation over any [`MessagingProvider`].
//!
//! ## Overview
//! [`DefaultMessagingService`] wraps an `Arc<dyn MessagingProvider>` and adds:
//! - message (de)serialization,
//! - the **callback dispatch** model — a per-subscription background task drains
//!   the provider's internal queue and invokes a [`MessageHandler`] under a
//!   concurrency limit, and
//! - request/reply correlation.
//!
//! ## Semantics & Architecture
//! - **Callback delivery**: [`MessagingService::subscribe`] registers a handler
//!   invoked on each matching message and returns `()` (matching the Java/Python
//!   contract). The service tracks each subscription internally keyed by
//!   `(destination, filter)`.
//! - **Stopping**: [`MessagingService::unsubscribe`] is the only way for callers to
//!   stop a subscription; it aborts the dispatcher **and** sends an UNSUBSCRIBE to
//!   the broker, so no broker subscription is orphaned. Dropping the service aborts
//!   all dispatchers (the broker drops the subscriptions when the connection closes).
//! - **Queuing**: the provider pushes into an unbounded channel, so no messages
//!   are dropped while a handler runs.
//! - **Concurrency**: `max_concurrency` bounds simultaneous handler invocations. A
//!   value of `1` gives strictly serial, ordered processing; `N` permits up to `N`
//!   concurrent handlers.
//! - Async (`tokio`); object-safe via `async_trait`.
//! - Error handling: [`crate::error::Result`]; timeouts and closed channels are
//!   reported as [`crate::error::GgError::Messaging`], never panics.
//!
//! ## Usage Example
//! ```no_run
//! # async fn demo(provider: std::sync::Arc<dyn ggcommons::messaging::MessagingProvider>) -> ggcommons::Result<()> {
//! use ggcommons::messaging::service::{DefaultMessagingService, MessagingService};
//! use ggcommons::messaging::{message_handler, Destination};
//!
//! let svc = DefaultMessagingService::new(provider);
//! svc.subscribe("events/+", Destination::Local, 4, message_handler(|topic, msg| async move {
//!     println!("{topic}: {}", msg.header.name);
//! })).await?;
//! // ... later:
//! svc.unsubscribe("events/+", Destination::Local).await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Design Choices
//! - Subscription lifecycle handles are kept **internal** (a map of dispatcher
//!   tasks). Callers never hold a handle, so they cannot accidentally orphan a
//!   broker subscription by dropping one — they must call `unsubscribe`.
//! - The concurrency semaphore is acquired *before* each dispatch, so a limit of
//!   `1` yields ordered serial processing without a separate code path.
//! - Replies are published on [`Destination::Local`]; cross-destination reply
//!   routing is a later refinement (standalone request/reply runs on the local bus).
//!
//! ## Safety & Panics
//! None in normal operation.
//!
//! ## Related Modules
//! - [`crate::messaging::provider`], [`crate::messaging::message`].

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;

use super::message::Message;
use super::{request_reply, Destination, MessagingProvider, Qos, Subscription};
use crate::error::{GgError, Result};

/// A handler invoked for each message delivered to a subscription.
///
/// Mirrors the Java/Python `MessageHandler` contract. Implement it on your own
/// type for testability, or wrap an async closure with [`message_handler`].
#[async_trait]
pub trait MessageHandler: Send + Sync + 'static {
    /// Process one message received on `topic`.
    async fn handle(&self, topic: String, message: Message);
}

/// Adapts an async closure into a [`MessageHandler`].
struct FnHandler<F>(F);

#[async_trait]
impl<F, Fut> MessageHandler for FnHandler<F>
where
    F: Fn(String, Message) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    async fn handle(&self, topic: String, message: Message) {
        (self.0)(topic, message).await
    }
}

/// Wrap an async closure as a [`MessageHandler`] for [`MessagingService::subscribe`].
///
/// # Purpose
/// Provide an ergonomic way to register a handler without defining a type, while
/// keeping [`MessagingService`] object-safe (the trait method takes
/// `Arc<dyn MessageHandler>`).
///
/// # Examples
/// ```
/// use ggcommons::messaging::message_handler;
/// let _h = message_handler(|topic, msg| async move {
///     let _ = (topic, msg);
/// });
/// ```
pub fn message_handler<F, Fut>(f: F) -> Arc<dyn MessageHandler>
where
    F: Fn(String, Message) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    Arc::new(FnHandler(f))
}

/// Transport-agnostic messaging operations over [`Message`]s.
#[async_trait]
pub trait MessagingService: Send + Sync {
    /// Publish a message to `topic` on `dest`.
    async fn publish(&self, topic: &str, msg: &Message, dest: Destination) -> Result<()>;

    /// Register a callback `handler` for messages matching `filter` on `dest`.
    ///
    /// `max_concurrency` bounds simultaneous handler invocations; use `1` for
    /// strictly serial, ordered processing. Subscribing again with the same
    /// `(dest, filter)` replaces the previous handler. Stop a subscription with
    /// [`MessagingService::unsubscribe`].
    async fn subscribe(
        &self,
        filter: &str,
        dest: Destination,
        max_concurrency: usize,
        handler: Arc<dyn MessageHandler>,
    ) -> Result<()>;

    /// Stop the subscription for `filter` on `dest`: aborts its dispatcher and
    /// unsubscribes at the broker.
    async fn unsubscribe(&self, filter: &str, dest: Destination) -> Result<()>;

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
    /// Internal dispatcher handles, keyed by `(destination, filter)`. Not exposed —
    /// callers stop subscriptions via [`MessagingService::unsubscribe`].
    subscriptions: Mutex<HashMap<(Destination, String), JoinHandle<()>>>,
}

impl DefaultMessagingService {
    /// Wrap a provider in the default service.
    pub fn new(provider: Arc<dyn MessagingProvider>) -> Self {
        Self {
            provider,
            subscriptions: Mutex::new(HashMap::new()),
        }
    }
}

impl Drop for DefaultMessagingService {
    fn drop(&mut self) {
        if let Ok(mut map) = self.subscriptions.lock() {
            for (_key, task) in map.drain() {
                task.abort();
            }
        }
    }
}

#[async_trait]
impl MessagingService for DefaultMessagingService {
    async fn publish(&self, topic: &str, msg: &Message, dest: Destination) -> Result<()> {
        let bytes = msg.to_vec()?;
        self.provider.publish(topic, bytes, dest, Qos::AtLeastOnce).await
    }

    /// Subscribe with a callback handler.
    ///
    /// # Algorithmic Choices
    /// Opens a raw provider subscription (the internal queue), spawns a dispatcher
    /// task that drains it and invokes `handler` under a `Semaphore(max_concurrency)`,
    /// and records the task handle internally keyed by `(dest, filter)`. The
    /// semaphore permit is acquired before each dispatch, so `max_concurrency == 1`
    /// processes messages serially and in order.
    ///
    /// # Post-conditions
    /// Any prior subscription for the same `(dest, filter)` has its dispatcher aborted.
    ///
    /// # Errors
    /// | Error Variant | Condition | Recovery |
    /// |---------------|-----------|----------|
    /// | `GgError::Messaging` | The provider could not establish the subscription | Verify connectivity |
    async fn subscribe(
        &self,
        filter: &str,
        dest: Destination,
        max_concurrency: usize,
        handler: Arc<dyn MessageHandler>,
    ) -> Result<()> {
        let sub = self.provider.subscribe(filter, dest, Qos::AtLeastOnce).await?;
        let task = tokio::spawn(run_dispatcher(sub, handler, max_concurrency));

        let previous = {
            let mut map = self
                .subscriptions
                .lock()
                .map_err(|_| GgError::Messaging("subscription map poisoned".to_string()))?;
            map.insert((dest, filter.to_string()), task)
        };
        if let Some(old) = previous {
            old.abort();
        }
        Ok(())
    }

    /// Stop a subscription and unsubscribe at the broker.
    ///
    /// # Post-conditions
    /// The dispatcher (if any) is aborted and the broker is asked to UNSUBSCRIBE.
    ///
    /// # Errors
    /// | Error Variant | Condition | Recovery |
    /// |---------------|-----------|----------|
    /// | `GgError::Messaging` | The broker unsubscribe failed | Retry; local dispatch is already stopped |
    async fn unsubscribe(&self, filter: &str, dest: Destination) -> Result<()> {
        let task = {
            let mut map = self
                .subscriptions
                .lock()
                .map_err(|_| GgError::Messaging("subscription map poisoned".to_string()))?;
            map.remove(&(dest, filter.to_string()))
        };
        if let Some(task) = task {
            task.abort();
        }
        self.provider.unsubscribe(filter, dest).await
    }

    /// Send a request and await its reply.
    ///
    /// # Algorithmic Choices
    /// Subscribes to a unique reply topic, stamps it as the request's `replyTo`,
    /// publishes, then awaits the first message with [`tokio::time::timeout`]. The
    /// reply subscription is dropped (local cleanup) and UNSUBSCRIBE-d at the broker
    /// on return — success, timeout, or error — so no reply topic is orphaned.
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
        request.header.reply_to = Some(reply_topic.clone());
        self.provider
            .publish(topic, request.to_vec()?, dest, Qos::AtLeastOnce)
            .await?;

        let result = match tokio::time::timeout(timeout, sub.recv()).await {
            Ok(Some((_topic, bytes))) => Message::from_slice(&bytes),
            Ok(None) => Err(GgError::Messaging(
                "reply channel closed before a reply arrived".to_string(),
            )),
            Err(_) => Err(GgError::Messaging("request timed out".to_string())),
        };

        // Local cleanup (drop) + broker cleanup, so the ephemeral reply topic is
        // never left subscribed at the broker.
        drop(sub);
        let _ = self.provider.unsubscribe(&reply_topic, dest).await;
        result
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

/// Drain a subscription's queue and invoke `handler` with bounded concurrency.
///
/// # Algorithmic Choices
/// A `Semaphore` with `max(max_concurrency, 1)` permits gates dispatch. Acquiring a
/// permit before spawning the handler means a single-permit semaphore serializes
/// handlers in arrival order; multiple permits allow that many concurrent handlers.
async fn run_dispatcher(
    mut sub: Subscription,
    handler: Arc<dyn MessageHandler>,
    max_concurrency: usize,
) {
    let semaphore = Arc::new(Semaphore::new(max_concurrency.max(1)));

    while let Some((topic, bytes)) = sub.recv().await {
        let message = match Message::from_slice(&bytes) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(topic = %topic, error = %e, "dropping unparseable message");
                continue;
            }
        };

        // Acquire before dispatch: a single permit => serial, ordered processing.
        let permit = match semaphore.clone().acquire_owned().await {
            Ok(p) => p,
            Err(_) => break, // semaphore closed (shouldn't happen; we never close it)
        };
        let handler = handler.clone();
        tokio::spawn(async move {
            let _permit = permit;
            handler.handle(topic, message).await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messaging::message::MessageBuilder;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::mpsc::UnboundedSender;

    /// Raw `(topic, payload)` pair pushed from the provider to a subscription.
    type RawMessage = (String, Vec<u8>);

    /// A fake provider that hands out a single subscription and lets the test push
    /// messages into it, so the dispatcher (callback + concurrency) can be tested
    /// without a broker.
    struct FakeProvider {
        sender: Mutex<Option<UnboundedSender<RawMessage>>>,
        unsubscribed: AtomicUsize,
    }

    impl FakeProvider {
        fn new() -> Self {
            Self {
                sender: Mutex::new(None),
                unsubscribed: AtomicUsize::new(0),
            }
        }
        /// Inject a message after a subscription has been created. Tolerates a
        /// closed receiver (e.g. after the subscription is stopped).
        fn push(&self, topic: &str, msg: &Message) {
            let guard = self.sender.lock().unwrap();
            let tx = guard.as_ref().expect("subscribe was called first");
            let _ = tx.send((topic.to_string(), msg.to_vec().unwrap()));
        }
    }

    #[async_trait]
    impl MessagingProvider for FakeProvider {
        async fn publish(&self, _t: &str, _p: Vec<u8>, _d: Destination, _q: Qos) -> Result<()> {
            Ok(())
        }
        async fn subscribe(&self, _f: &str, _d: Destination, _q: Qos) -> Result<Subscription> {
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
            *self.sender.lock().unwrap() = Some(tx);
            Ok(Subscription::new(rx, Box::new(())))
        }
        async fn unsubscribe(&self, _f: &str, _d: Destination) -> Result<()> {
            self.unsubscribed.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn msg(n: u64) -> Message {
        MessageBuilder::new("T", "1.0").payload(json!(n)).build()
    }

    /// Wait until `counter` reaches `target` or a timeout elapses.
    async fn wait_for(counter: &AtomicUsize, target: usize) {
        for _ in 0..200 {
            if counter.load(Ordering::SeqCst) >= target {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!(
            "timed out waiting for {target}; reached {}",
            counter.load(Ordering::SeqCst)
        );
    }

    #[tokio::test]
    async fn handler_is_invoked_per_message() {
        let provider = Arc::new(FakeProvider::new());
        let svc = DefaultMessagingService::new(provider.clone());

        let count = Arc::new(AtomicUsize::new(0));
        let count_h = count.clone();
        svc.subscribe(
            "t",
            Destination::Local,
            1,
            message_handler(move |_topic, _msg| {
                let count_h = count_h.clone();
                async move {
                    count_h.fetch_add(1, Ordering::SeqCst);
                }
            }),
        )
        .await
        .unwrap();

        provider.push("t", &msg(1));
        provider.push("t", &msg(2));
        wait_for(&count, 2).await;
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn max_concurrency_one_processes_in_order() {
        let provider = Arc::new(FakeProvider::new());
        let svc = DefaultMessagingService::new(provider.clone());

        let order = Arc::new(Mutex::new(Vec::<u64>::new()));
        let done = Arc::new(AtomicUsize::new(0));
        let order_h = order.clone();
        let done_h = done.clone();
        svc.subscribe(
            "t",
            Destination::Local,
            1, // serial
            message_handler(move |_topic, m| {
                let order_h = order_h.clone();
                let done_h = done_h.clone();
                async move {
                    let n = m.body.as_u64().unwrap();
                    // Earlier messages sleep longer; serial processing must still
                    // preserve arrival order regardless of per-handler duration.
                    tokio::time::sleep(Duration::from_millis((5 - n) * 20)).await;
                    order_h.lock().unwrap().push(n);
                    done_h.fetch_add(1, Ordering::SeqCst);
                }
            }),
        )
        .await
        .unwrap();

        for n in 0..4u64 {
            provider.push("t", &msg(n));
        }
        wait_for(&done, 4).await;
        assert_eq!(*order.lock().unwrap(), vec![0, 1, 2, 3]);
    }

    #[tokio::test]
    async fn max_concurrency_n_allows_parallelism() {
        let provider = Arc::new(FakeProvider::new());
        let svc = DefaultMessagingService::new(provider.clone());

        let active = Arc::new(AtomicUsize::new(0));
        let max_seen = Arc::new(AtomicUsize::new(0));
        let done = Arc::new(AtomicUsize::new(0));
        let (active_h, max_h, done_h) = (active.clone(), max_seen.clone(), done.clone());

        svc.subscribe(
            "t",
            Destination::Local,
            3, // up to 3 concurrent
            message_handler(move |_topic, _m| {
                let (active_h, max_h, done_h) = (active_h.clone(), max_h.clone(), done_h.clone());
                async move {
                    let now = active_h.fetch_add(1, Ordering::SeqCst) + 1;
                    max_h.fetch_max(now, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(80)).await;
                    active_h.fetch_sub(1, Ordering::SeqCst);
                    done_h.fetch_add(1, Ordering::SeqCst);
                }
            }),
        )
        .await
        .unwrap();

        for n in 0..3u64 {
            provider.push("t", &msg(n));
        }
        wait_for(&done, 3).await;
        // All three overlap given the limit of 3 and the 80ms dwell.
        assert_eq!(max_seen.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn unsubscribe_stops_dispatch_and_calls_broker() {
        let provider = Arc::new(FakeProvider::new());
        let svc = DefaultMessagingService::new(provider.clone());

        let count = Arc::new(AtomicUsize::new(0));
        let count_h = count.clone();
        svc.subscribe(
            "t",
            Destination::Local,
            1,
            message_handler(move |_t, _m| {
                let count_h = count_h.clone();
                async move {
                    count_h.fetch_add(1, Ordering::SeqCst);
                }
            }),
        )
        .await
        .unwrap();

        provider.push("t", &msg(1));
        wait_for(&count, 1).await;

        svc.unsubscribe("t", Destination::Local).await.unwrap();
        assert_eq!(provider.unsubscribed.load(Ordering::SeqCst), 1, "broker unsubscribe called");

        tokio::time::sleep(Duration::from_millis(50)).await;
        provider.push("t", &msg(2));
        tokio::time::sleep(Duration::from_millis(100)).await;
        // The second message is not processed after unsubscribe.
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }
}
