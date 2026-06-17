//! # Messaging — service layer
//!
//! **One-liner purpose**: The user-facing [`MessagingService`] — explicit local /
//! IoT Core method pairs for publish, subscribe, request/reply — over any
//! [`MessagingProvider`].
//!
//! ## Overview
//! [`DefaultMessagingService`] wraps an `Arc<dyn MessagingProvider>` and adds
//! message (de)serialization, the callback dispatch model, and request/reply
//! correlation. The surface mirrors the Java/Python `IMessagingService`:
//! `publish`/`publish_to_iot_core`, `publish_raw`/`publish_to_iot_core_raw`,
//! `subscribe`/`subscribe_to_iot_core`, `unsubscribe`/`unsubscribe_from_iot_core`,
//! `request`/`request_from_iot_core`, `reply`/`reply_to_iot_core`,
//! `cancel_request`/`cancel_request_from_iot_core`.
//!
//! ## Semantics & Architecture
//! - **Callback delivery**: `subscribe*` registers a [`MessageHandler`] and returns
//!   `()`; the service tracks each subscription internally by `(destination, filter)`.
//! - **Two settings**: `max_messages` bounds the client-side queue (the provider
//!   drops on overflow with a warning); `max_concurrency` bounds simultaneous
//!   handler invocations (`1` = serial, ordered).
//! - **Stopping**: only `unsubscribe*` stops a subscription (aborts the dispatcher
//!   AND UNSUBSCRIBEs at the broker). Dropping the service stops all dispatchers.
//! - **Request/reply**: `request*` returns a [`ReplyFuture`] (await it, or wrap in
//!   `tokio::time::timeout`); `cancel_request*` abandons it. All paths
//!   (completion, timeout, cancel) UNSUBSCRIBE the ephemeral reply topic.
//! - Async (`tokio`); object-safe via `async_trait`.
//! - Error handling: [`crate::error::Result`]; never panics.
//!
//! ## Usage Example
//! ```no_run
//! # async fn demo(provider: std::sync::Arc<dyn ggcommons::messaging::MessagingProvider>) -> ggcommons::Result<()> {
//! use ggcommons::messaging::service::{DefaultMessagingService, MessagingService};
//! use ggcommons::messaging::{message_handler, message::MessageBuilder};
//! use std::time::Duration;
//!
//! let svc = DefaultMessagingService::new(provider);
//! svc.subscribe("events/+", message_handler(|t, m| async move {
//!     println!("{t}: {}", m.header.name);
//! }), 32, 4).await?;
//!
//! let reply = tokio::time::timeout(
//!     Duration::from_secs(5),
//!     svc.request("svc/ping", MessageBuilder::new("Ping", "1.0").thing_name("t").build()).await?,
//! ).await;
//! # let _ = reply;
//! # Ok(())
//! # }
//! ```
//!
//! ## Design Choices
//! - Subscription lifecycle handles are kept internal; callers stop via
//!   `unsubscribe*` so a broker subscription can't be orphaned.
//! - `request*` returns a `ReplyFuture` (Rust analog of Java `CompletableFuture` /
//!   Python `Iou`) rather than taking a timeout argument, matching both libraries.
//!
//! ## Safety & Panics
//! None in normal operation.
//!
//! ## Related Modules
//! - [`crate::messaging::provider`], [`crate::messaging::message`].

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;

use super::message::Message;
use super::{request_reply, Destination, MessagingProvider, Qos, Subscription};
use crate::error::{GgError, Result};

/// Default QoS for local (IPC-style) operations, which have no explicit QoS in the
/// Java/Python contract.
const LOCAL_QOS: Qos = Qos::AtLeastOnce;
/// Reply subscriptions only ever receive one message.
const REPLY_QUEUE_SIZE: usize = 1;

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

/// Wrap an async closure as a [`MessageHandler`] for `subscribe*`.
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

/// A pending request's reply, the Rust analog of Java's `CompletableFuture<Message>`
/// / Python's `Iou`.
///
/// Await it directly for the reply, or wrap it in `tokio::time::timeout` for a
/// deadline. Completing, timing out (the outer future is dropped), or passing it to
/// [`MessagingService::cancel_request`] all UNSUBSCRIBE the ephemeral reply topic at
/// the broker, so no reply subscription is orphaned.
pub struct ReplyFuture {
    sub: Subscription,
    provider: Arc<dyn MessagingProvider>,
    reply_topic: String,
    dest: Destination,
}

impl Future for ReplyFuture {
    type Output = Result<Message>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut(); // ReplyFuture is Unpin
        match this.sub.poll_recv(cx) {
            Poll::Ready(Some((_topic, bytes))) => Poll::Ready(Message::from_slice(&bytes)),
            Poll::Ready(None) => Poll::Ready(Err(GgError::Messaging(
                "reply channel closed before a reply arrived".to_string(),
            ))),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for ReplyFuture {
    fn drop(&mut self) {
        // Best-effort broker cleanup on completion, timeout, or cancel. (Local queue
        // routing is removed by the Subscription's own Drop.)
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let provider = self.provider.clone();
            let topic = self.reply_topic.clone();
            let dest = self.dest;
            handle.spawn(async move {
                let _ = provider.unsubscribe(&topic, dest).await;
            });
        }
    }
}

/// Transport-agnostic messaging operations over [`Message`]s, with explicit local /
/// IoT Core method pairs (mirroring the Java/Python `IMessagingService`).
#[async_trait]
pub trait MessagingService: Send + Sync {
    /// Publish a message to `topic` on the local broker.
    async fn publish(&self, topic: &str, msg: &Message) -> Result<()>;
    /// Publish a message to `topic` on AWS IoT Core at `qos`.
    async fn publish_to_iot_core(&self, topic: &str, msg: &Message, qos: Qos) -> Result<()>;

    /// Publish a raw JSON payload to `topic` on the local broker.
    async fn publish_raw(&self, topic: &str, payload: &Value) -> Result<()>;
    /// Publish a raw JSON payload to `topic` on AWS IoT Core at `qos`.
    async fn publish_to_iot_core_raw(&self, topic: &str, payload: &Value, qos: Qos) -> Result<()>;

    /// Register a callback for `filter` on the local broker.
    ///
    /// `max_messages` bounds the client-side queue; `max_concurrency` bounds
    /// simultaneous handler invocations (`1` = serial, ordered).
    async fn subscribe(
        &self,
        filter: &str,
        handler: Arc<dyn MessageHandler>,
        max_messages: usize,
        max_concurrency: usize,
    ) -> Result<()>;

    /// Register a callback for `filter` on AWS IoT Core at `qos`.
    async fn subscribe_to_iot_core(
        &self,
        filter: &str,
        handler: Arc<dyn MessageHandler>,
        qos: Qos,
        max_messages: usize,
        max_concurrency: usize,
    ) -> Result<()>;

    /// Stop the local subscription for `filter` (aborts dispatch + broker UNSUBSCRIBE).
    async fn unsubscribe(&self, filter: &str) -> Result<()>;
    /// Stop the IoT Core subscription for `filter`.
    async fn unsubscribe_from_iot_core(&self, filter: &str) -> Result<()>;

    /// Send a request on the local broker; await the returned [`ReplyFuture`].
    async fn request(&self, topic: &str, msg: Message) -> Result<ReplyFuture>;
    /// Send a request on AWS IoT Core; await the returned [`ReplyFuture`].
    async fn request_from_iot_core(&self, topic: &str, msg: Message) -> Result<ReplyFuture>;

    /// Reply to a received request on the local broker.
    async fn reply(&self, request: &Message, reply: Message) -> Result<()>;
    /// Reply to a received request on AWS IoT Core.
    async fn reply_to_iot_core(&self, request: &Message, reply: Message) -> Result<()>;

    /// Abandon a pending local request, cleaning up its reply subscription.
    fn cancel_request(&self, reply_future: ReplyFuture);
    /// Abandon a pending IoT Core request, cleaning up its reply subscription.
    fn cancel_request_from_iot_core(&self, reply_future: ReplyFuture);
}

/// Default [`MessagingService`] built over a [`MessagingProvider`].
pub struct DefaultMessagingService {
    provider: Arc<dyn MessagingProvider>,
    /// Internal dispatcher handles, keyed by `(destination, filter)`. Not exposed —
    /// callers stop subscriptions via `unsubscribe*`.
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

    /// Open a provider subscription, spawn its dispatcher, and record the handle.
    async fn start_subscription(
        &self,
        filter: &str,
        dest: Destination,
        qos: Qos,
        max_messages: usize,
        max_concurrency: usize,
        handler: Arc<dyn MessageHandler>,
    ) -> Result<()> {
        let sub = self.provider.subscribe(filter, dest, qos, max_messages).await?;
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

    /// Abort the dispatcher (if any) and UNSUBSCRIBE at the broker.
    async fn stop_subscription(&self, filter: &str, dest: Destination) -> Result<()> {
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

    /// Issue a request on `dest` and return its reply handle.
    async fn start_request(
        &self,
        topic: &str,
        msg: Message,
        dest: Destination,
        qos: Qos,
    ) -> Result<ReplyFuture> {
        let reply_topic = request_reply::new_reply_topic();
        let sub = self
            .provider
            .subscribe(&reply_topic, dest, qos, REPLY_QUEUE_SIZE)
            .await?;

        let mut request = msg;
        request.header.reply_to = Some(reply_topic.clone());
        self.provider
            .publish(topic, request.to_vec()?, dest, qos)
            .await?;

        Ok(ReplyFuture {
            sub,
            provider: self.provider.clone(),
            reply_topic,
            dest,
        })
    }

    /// Publish a reply correlated with `request` on `dest`.
    async fn send_reply(&self, request: &Message, reply: Message, dest: Destination) -> Result<()> {
        let topic = request.header.reply_to.clone().ok_or_else(|| {
            GgError::Messaging("cannot reply: request has no reply_to".to_string())
        })?;
        let mut reply = reply;
        reply.header.correlation_id = request.header.correlation_id.clone();
        self.provider
            .publish(&topic, reply.to_vec()?, dest, LOCAL_QOS)
            .await
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
    async fn publish(&self, topic: &str, msg: &Message) -> Result<()> {
        self.provider
            .publish(topic, msg.to_vec()?, Destination::Local, LOCAL_QOS)
            .await
    }

    async fn publish_to_iot_core(&self, topic: &str, msg: &Message, qos: Qos) -> Result<()> {
        self.provider
            .publish(topic, msg.to_vec()?, Destination::IotCore, qos)
            .await
    }

    async fn publish_raw(&self, topic: &str, payload: &Value) -> Result<()> {
        self.provider
            .publish(topic, serde_json::to_vec(payload)?, Destination::Local, LOCAL_QOS)
            .await
    }

    async fn publish_to_iot_core_raw(&self, topic: &str, payload: &Value, qos: Qos) -> Result<()> {
        self.provider
            .publish(topic, serde_json::to_vec(payload)?, Destination::IotCore, qos)
            .await
    }

    async fn subscribe(
        &self,
        filter: &str,
        handler: Arc<dyn MessageHandler>,
        max_messages: usize,
        max_concurrency: usize,
    ) -> Result<()> {
        self.start_subscription(
            filter,
            Destination::Local,
            LOCAL_QOS,
            max_messages,
            max_concurrency,
            handler,
        )
        .await
    }

    async fn subscribe_to_iot_core(
        &self,
        filter: &str,
        handler: Arc<dyn MessageHandler>,
        qos: Qos,
        max_messages: usize,
        max_concurrency: usize,
    ) -> Result<()> {
        self.start_subscription(
            filter,
            Destination::IotCore,
            qos,
            max_messages,
            max_concurrency,
            handler,
        )
        .await
    }

    async fn unsubscribe(&self, filter: &str) -> Result<()> {
        self.stop_subscription(filter, Destination::Local).await
    }

    async fn unsubscribe_from_iot_core(&self, filter: &str) -> Result<()> {
        self.stop_subscription(filter, Destination::IotCore).await
    }

    async fn request(&self, topic: &str, msg: Message) -> Result<ReplyFuture> {
        self.start_request(topic, msg, Destination::Local, LOCAL_QOS).await
    }

    async fn request_from_iot_core(&self, topic: &str, msg: Message) -> Result<ReplyFuture> {
        self.start_request(topic, msg, Destination::IotCore, Qos::AtLeastOnce)
            .await
    }

    async fn reply(&self, request: &Message, reply: Message) -> Result<()> {
        self.send_reply(request, reply, Destination::Local).await
    }

    async fn reply_to_iot_core(&self, request: &Message, reply: Message) -> Result<()> {
        self.send_reply(request, reply, Destination::IotCore).await
    }

    fn cancel_request(&self, reply_future: ReplyFuture) {
        drop(reply_future); // Drop runs the broker cleanup.
    }

    fn cancel_request_from_iot_core(&self, reply_future: ReplyFuture) {
        drop(reply_future);
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

        let permit = match semaphore.clone().acquire_owned().await {
            Ok(p) => p,
            Err(_) => break,
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
    use std::time::Duration;
    use tokio::sync::mpsc::Sender;

    /// Raw `(topic, payload)` pair pushed from the provider to a subscription.
    type RawMessage = (String, Vec<u8>);

    /// A fake provider that hands out a single subscription and lets the test push
    /// messages into it, so the dispatcher can be tested without a broker.
    struct FakeProvider {
        sender: Mutex<Option<Sender<RawMessage>>>,
        unsubscribed: AtomicUsize,
    }

    impl FakeProvider {
        fn new() -> Self {
            Self {
                sender: Mutex::new(None),
                unsubscribed: AtomicUsize::new(0),
            }
        }
        fn push(&self, topic: &str, msg: &Message) {
            let guard = self.sender.lock().unwrap();
            let tx = guard.as_ref().expect("subscribe was called first");
            let _ = tx.try_send((topic.to_string(), msg.to_vec().unwrap()));
        }
    }

    #[async_trait]
    impl MessagingProvider for FakeProvider {
        async fn publish(&self, _t: &str, _p: Vec<u8>, _d: Destination, _q: Qos) -> Result<()> {
            Ok(())
        }
        async fn subscribe(
            &self,
            _f: &str,
            _d: Destination,
            _q: Qos,
            max_messages: usize,
        ) -> Result<Subscription> {
            let (tx, rx) = tokio::sync::mpsc::channel(max_messages.max(1));
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
            message_handler(move |_t, _m| {
                let count_h = count_h.clone();
                async move {
                    count_h.fetch_add(1, Ordering::SeqCst);
                }
            }),
            16,
            1,
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
            message_handler(move |_t, m| {
                let order_h = order_h.clone();
                let done_h = done_h.clone();
                async move {
                    let n = m.body.as_u64().unwrap();
                    tokio::time::sleep(Duration::from_millis((5 - n) * 20)).await;
                    order_h.lock().unwrap().push(n);
                    done_h.fetch_add(1, Ordering::SeqCst);
                }
            }),
            16,
            1, // serial
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
            message_handler(move |_t, _m| {
                let (active_h, max_h, done_h) = (active_h.clone(), max_h.clone(), done_h.clone());
                async move {
                    let now = active_h.fetch_add(1, Ordering::SeqCst) + 1;
                    max_h.fetch_max(now, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(80)).await;
                    active_h.fetch_sub(1, Ordering::SeqCst);
                    done_h.fetch_add(1, Ordering::SeqCst);
                }
            }),
            16,
            3, // up to 3 concurrent
        )
        .await
        .unwrap();

        for n in 0..3u64 {
            provider.push("t", &msg(n));
        }
        wait_for(&done, 3).await;
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
            message_handler(move |_t, _m| {
                let count_h = count_h.clone();
                async move {
                    count_h.fetch_add(1, Ordering::SeqCst);
                }
            }),
            16,
            1,
        )
        .await
        .unwrap();

        provider.push("t", &msg(1));
        wait_for(&count, 1).await;

        svc.unsubscribe("t").await.unwrap();
        assert_eq!(provider.unsubscribed.load(Ordering::SeqCst), 1, "broker unsubscribe called");

        tokio::time::sleep(Duration::from_millis(50)).await;
        provider.push("t", &msg(2));
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn request_delivers_reply_and_unsubscribes_reply_topic() {
        let provider = Arc::new(FakeProvider::new());
        let svc = DefaultMessagingService::new(provider.clone());

        let reply_future = svc.request("svc/op", msg(1)).await.unwrap();
        // Simulate the reply arriving on the ephemeral reply topic.
        provider.push("reply", &msg(99));

        let reply = reply_future.await.unwrap();
        assert_eq!(reply.body.as_u64().unwrap(), 99);

        // Completing the request drops the ReplyFuture, which UNSUBSCRIBEs the
        // ephemeral reply topic at the broker (no orphan).
        wait_for(&provider.unsubscribed, 1).await;
    }

    #[tokio::test]
    async fn cancel_request_unsubscribes_reply_topic() {
        let provider = Arc::new(FakeProvider::new());
        let svc = DefaultMessagingService::new(provider.clone());

        let reply_future = svc.request("svc/op", msg(1)).await.unwrap();
        // No reply pushed; abandon it.
        svc.cancel_request(reply_future);

        // Cancellation cleans up the reply subscription at the broker.
        wait_for(&provider.unsubscribed, 1).await;
    }

    #[tokio::test]
    async fn request_timeout_unsubscribes_reply_topic() {
        let provider = Arc::new(FakeProvider::new());
        let svc = DefaultMessagingService::new(provider.clone());

        let reply_future = svc.request("svc/op", msg(1)).await.unwrap();
        // No reply pushed; the await times out and drops the ReplyFuture.
        let result = tokio::time::timeout(Duration::from_millis(50), reply_future).await;
        assert!(result.is_err(), "expected the await to time out");

        // Timing out cleans up the reply subscription at the broker.
        wait_for(&provider.unsubscribed, 1).await;
    }
}
