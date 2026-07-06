//! # Messaging — service layer
//!
//! **One-liner purpose**: The user-facing [`MessagingService`] — explicit local /
//! IoT Core method pairs for publish, subscribe, request/reply — over any
//! [`MessagingProvider`], with the UNS reserved-class publish guard and the
//! framework-owned request deadline.
//!
//! ## Overview
//! [`DefaultMessagingService`] wraps an `Arc<dyn MessagingProvider>` and adds
//! message (de)serialization, the callback dispatch model, request/reply
//! correlation, the reserved-class publish guard (UNS-CANONICAL-DESIGN §4.1), and
//! the `request()` deadline (§5). The surface mirrors the Java `MessagingClient`:
//! `publish`/`publish_to_iot_core`, `publish_raw`/`publish_to_iot_core_raw`,
//! `subscribe`/`subscribe_to_iot_core`, `unsubscribe`/`unsubscribe_from_iot_core`,
//! `request`/`request_from_iot_core` (+ `_with_timeout` variants),
//! `reply`/`reply_to_iot_core`, `cancel_request`/`cancel_request_from_iot_core`.
//!
//! ## Reserved-class publish guard (§4.1, D-U4/D-U8/D-U24/D-U27)
//! Every public path that emits a **client-chosen topic** — `publish*`, `request*`,
//! and `reply*` (a hostile requester could set `header.reply_to` to a victim's
//! reserved topic) — rejects topics targeting a library-owned UNS class
//! (`state | metric | cfg | log`) with [`EdgeCommonsError::ReservedTopic`]. `subscribe*` is
//! never guarded (consumers must read reserved classes); non-`ecv1` topics pass
//! untouched. The guard's `includeRoot` flag is late-bound to the **effective**
//! root ([`DefaultMessagingService::set_guard_include_root`], D-U27) once the
//! configuration exists.
//!
//! The library's own publishers (heartbeat `state` keepalive, the `messaging`
//! metric target, the effective-config `cfg` publisher) reach the reserved classes
//! through the crate-private [`ReservedMessaging`] seam (§4.2, D-U4) — the only
//! compiler-enforced privileged seam across the four language libraries.
//!
//! ## Request deadline (§5, D-U5)
//! `request*` arms a **framework-owned deadline at send time** (default
//! `messaging.requestTimeoutSeconds`, built-in 30 s until late-bound). Each request
//! is owned by a **spawned supervisor task** that holds the ephemeral reply
//! subscription and `select!`s over reply / deadline / cancel — the single
//! idempotent settle site. On ANY settle it unsubscribes the reply topic and sends
//! the outcome down a oneshot, so the deadline fires cleanup (and the
//! [`EdgeCommonsError::RequestTimeout`] error) **even if the returned [`ReplyFuture`] is
//! never polled** — the reply-subscription-leak fix. Dropping the `ReplyFuture`
//! still cancels the request (today's contract, preserved).
//!
//! ## Related Modules
//! - [`crate::messaging::provider`], [`crate::messaging::message`], [`crate::uns`].

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::{oneshot, Semaphore};
use tokio::task::JoinHandle;

use super::message::Message;
use super::{request_reply, Destination, MessagingProvider, Qos, Subscription};
use crate::error::{EdgeCommonsError, Result};

/// Default QoS for local (IPC-style) operations, which have no explicit QoS in the
/// Java/Python contract.
const LOCAL_QOS: Qos = Qos::AtLeastOnce;
/// Reply subscriptions only ever receive one message.
const REPLY_QUEUE_SIZE: usize = 1;
/// The built-in `request()` deadline (ms) that applies until the config-model
/// default (`messaging.requestTimeoutSeconds`) is late-bound. Deliberately non-zero
/// so the CONFIG_COMPONENT bootstrap request gets a deadline instead of hanging.
const BUILT_IN_REQUEST_TIMEOUT_MS: u64 = 30_000;

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
/// use edgecommons::messaging::message_handler;
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

/// A pending request's reply, the Rust analog of Java's
/// `ReplyFuture extends CompletableFuture<Message>` / Python's `Iou`.
///
/// The request is owned by a spawned **supervisor task** (see the module docs):
/// this handle wraps the supervisor's result oneshot plus a cancel handle. Await it
/// for the reply; on the framework deadline it resolves
/// `Err(`[`EdgeCommonsError::RequestTimeout`]`)`. Dropping it (or passing it to
/// [`MessagingService::cancel_request`]) cancels the request; every settle path —
/// reply, deadline, cancel — UNSUBSCRIBEs the ephemeral reply topic at the broker
/// exactly once, so no reply subscription is orphaned **even if this future is
/// never polled**.
pub struct ReplyFuture {
    /// The supervisor's settled outcome.
    rx: oneshot::Receiver<Result<Message>>,
    /// Cancel handle: consumed (or dropped) to signal the supervisor. Dropping the
    /// sender is itself the cancel signal, so `Drop` needs no explicit send.
    cancel: Option<oneshot::Sender<()>>,
}

impl Future for ReplyFuture {
    type Output = Result<Message>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut(); // ReplyFuture is Unpin
        match Pin::new(&mut this.rx).poll(cx) {
            Poll::Ready(Ok(outcome)) => Poll::Ready(outcome),
            // The supervisor vanished without settling (runtime shutdown).
            Poll::Ready(Err(_)) => Poll::Ready(Err(EdgeCommonsError::Messaging(
                "request supervisor ended before a reply arrived".to_string(),
            ))),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for ReplyFuture {
    fn drop(&mut self) {
        // Dropping the cancel sender resolves the supervisor's cancel arm, which
        // unsubscribes the reply topic (idempotent: a no-op if already settled).
        drop(self.cancel.take());
    }
}

/// The privileged internal-publish seam (UNS-CANONICAL-DESIGN §4.2, D-U4): the
/// library's own publishers — the heartbeat `state` keepalive, the `messaging`
/// metric target, and the effective-config (`cfg`) publisher — publish reserved
/// UNS classes through this **crate-private** trait, which BYPASSES the
/// reserved-class guard. Being `pub(crate)`, component code cannot name it: Rust is
/// the only language where the seam is compiler-enforced. Test fakes implement both
/// this and [`MessagingService`].
#[async_trait]
pub(crate) trait ReservedMessaging: Send + Sync {
    /// Publish a message to a reserved topic on the local broker, without the guard.
    async fn publish_reserved(&self, topic: &str, msg: &Message) -> Result<()>;

    /// Publish a message to a reserved topic on AWS IoT Core, without the guard.
    async fn publish_reserved_to_iot_core(
        &self,
        topic: &str,
        msg: &Message,
        qos: Qos,
    ) -> Result<()>;
}

/// Transport-agnostic messaging operations over [`Message`]s, with explicit local /
/// IoT Core method pairs (mirroring the Java/Python `IMessagingService`).
///
/// Client-chosen publish topics are subject to the reserved-class guard (see the
/// [module docs](self)); `subscribe*` is never guarded.
#[async_trait]
pub trait MessagingService: Send + Sync {
    /// Publish a message to `topic` on the local broker.
    ///
    /// # Errors
    /// [`EdgeCommonsError::ReservedTopic`] when `topic` targets a reserved UNS class (§4.1).
    async fn publish(&self, topic: &str, msg: &Message) -> Result<()>;
    /// Publish a message to `topic` on AWS IoT Core at `qos` (guarded like
    /// [`Self::publish`]).
    async fn publish_to_iot_core(&self, topic: &str, msg: &Message, qos: Qos) -> Result<()>;

    /// Publish a raw JSON payload to `topic` on the local broker (guarded — D-U8).
    async fn publish_raw(&self, topic: &str, payload: &Value) -> Result<()>;
    /// Publish a raw JSON payload to `topic` on AWS IoT Core at `qos` (guarded).
    async fn publish_to_iot_core_raw(&self, topic: &str, payload: &Value, qos: Qos) -> Result<()>;

    /// Register a callback for `filter` on the local broker.
    ///
    /// `max_messages` bounds the client-side queue; `max_concurrency` bounds
    /// simultaneous handler invocations (`1` = serial, ordered). Never guarded.
    async fn subscribe(
        &self,
        filter: &str,
        handler: Arc<dyn MessageHandler>,
        max_messages: usize,
        max_concurrency: usize,
    ) -> Result<()>;

    /// Register a callback for `filter` on AWS IoT Core at `qos`. Never guarded.
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
    ///
    /// Carries the framework-owned default deadline (`messaging.requestTimeoutSeconds`,
    /// default 30 s, UNS-CANONICAL-DESIGN §5): on expiry the ephemeral reply
    /// subscription is cleaned up and the future resolves
    /// `Err(`[`EdgeCommonsError::RequestTimeout`]`)` — even if it is never polled.
    ///
    /// # Errors
    /// [`EdgeCommonsError::ReservedTopic`] when `topic` targets a reserved UNS class.
    async fn request(&self, topic: &str, msg: Message) -> Result<ReplyFuture>;
    /// Send a request on AWS IoT Core; await the returned [`ReplyFuture`]. Same
    /// deadline + guard semantics as [`Self::request`].
    async fn request_from_iot_core(&self, topic: &str, msg: Message) -> Result<ReplyFuture>;

    /// [`Self::request`] with an explicit per-call deadline (§5, D-U5): an explicit
    /// value always wins over the configured default; `None` uses the default;
    /// `Some(Duration::ZERO)` disables the deadline for this call.
    async fn request_with_timeout(
        &self,
        topic: &str,
        msg: Message,
        timeout: Option<Duration>,
    ) -> Result<ReplyFuture>;
    /// IoT Core variant of [`Self::request_with_timeout`].
    async fn request_from_iot_core_with_timeout(
        &self,
        topic: &str,
        msg: Message,
        timeout: Option<Duration>,
    ) -> Result<ReplyFuture>;

    /// Reply to a received request on the local broker. The request's `reply_to`
    /// topic is guarded like a client-chosen topic (§4.1, D-U8): a hostile
    /// requester could otherwise set it to a victim's reserved topic and turn an
    /// innocent responder into a forger.
    async fn reply(&self, request: &Message, reply: Message) -> Result<()>;
    /// Reply to a received request on AWS IoT Core (guarded the same way).
    async fn reply_to_iot_core(&self, request: &Message, reply: Message) -> Result<()>;

    /// Abandon a pending local request, cleaning up its reply subscription.
    fn cancel_request(&self, reply_future: ReplyFuture);
    /// Abandon a pending IoT Core request, cleaning up its reply subscription.
    fn cancel_request_from_iot_core(&self, reply_future: ReplyFuture);

    /// Whether the messaging transport currently has a live connection.
    ///
    /// Delegates to the underlying [`MessagingProvider::connected`] (the local broker's MQTT
    /// CONNACK state, or `true` once the Greengrass IPC client is built). Used by the health
    /// readiness endpoint (`/readyz`); never used to gate liveness.
    fn connected(&self) -> bool;
}

/// Default [`MessagingService`] built over a [`MessagingProvider`].
pub struct DefaultMessagingService {
    provider: Arc<dyn MessagingProvider>,
    /// Internal dispatcher handles, keyed by `(destination, filter)`. Not exposed —
    /// callers stop subscriptions via `unsubscribe*`.
    subscriptions: Mutex<HashMap<(Destination, String), JoinHandle<()>>>,
    /// The default `request()` deadline in milliseconds; `0` = disabled. Starts at
    /// the built-in 30 s; late-bound from `messaging.requestTimeoutSeconds` via
    /// [`Self::set_default_request_timeout`] once the config exists (§5/D-U5).
    default_request_timeout_ms: AtomicU64,
    /// Whether the reserved-class guard also checks the class token at topic
    /// position 5 — this component's EFFECTIVE `topic.includeRoot`
    /// (UNS-CANONICAL-DESIGN §4.1, D-U24/D-U27). Late-bound via
    /// [`Self::set_guard_include_root`]; `false` pre-bind — nothing publishes
    /// rooted topics pre-config.
    guard_include_root: AtomicBool,
}

impl DefaultMessagingService {
    /// Wrap a provider in the default service.
    pub fn new(provider: Arc<dyn MessagingProvider>) -> Self {
        Self {
            provider,
            subscriptions: Mutex::new(HashMap::new()),
            default_request_timeout_ms: AtomicU64::new(BUILT_IN_REQUEST_TIMEOUT_MS),
            guard_include_root: AtomicBool::new(false),
        }
    }

    /// Late-binds the default `request()` deadline from the config model
    /// (`messaging.requestTimeoutSeconds`, §5/D-U5). Called by the runtime right
    /// after the configuration loads (the messaging service is constructed first
    /// because the CONFIG_COMPONENT source needs it); until then the built-in 30 s
    /// applies — deliberately, so the bootstrap request gets a deadline instead of
    /// hanging. `None` or a zero duration disables the default deadline.
    pub fn set_default_request_timeout(&self, timeout: Option<Duration>) {
        let ms = timeout.map(|d| d.as_millis().min(u128::from(u64::MAX)) as u64).unwrap_or(0);
        self.default_request_timeout_ms.store(ms, Ordering::Relaxed);
        tracing::debug!(timeout_ms = ms, "default request timeout bound (0 = disabled)");
    }

    /// The default `request()` deadline currently in effect (`None` = disabled).
    pub fn default_request_timeout(&self) -> Option<Duration> {
        match self.default_request_timeout_ms.load(Ordering::Relaxed) {
            0 => None,
            ms => Some(Duration::from_millis(ms)),
        }
    }

    /// Late-binds the reserved-class guard's `topic.includeRoot` flag (§4.1,
    /// D-U24). Bind the **effective** root — `includeRoot && hier.len() >= 2`
    /// (D-U27) — so the guard's position-5 check agrees with topic building, which
    /// no-ops includeRoot on a single-level hierarchy (D-U25). Before the bind only
    /// the always-checked class position 4 applies.
    pub fn set_guard_include_root(&self, include_root: bool) {
        self.guard_include_root.store(include_root, Ordering::Relaxed);
        tracing::debug!(include_root, "reserved-topic guard includeRoot bound");
    }

    /// The §4.1 reserved-class publish guard: rejects a client-chosen topic whose
    /// class position holds a reserved token (`state | metric | cfg | log`).
    /// `None` topics pass (no reply_to — provider-level validation owns that).
    fn check_reserved(&self, topic: Option<&str>) -> Result<()> {
        let Some(topic) = topic else { return Ok(()) };
        let include_root = self.guard_include_root.load(Ordering::Relaxed);
        if let Some(cls) = crate::uns::reserved_class_of(topic, include_root) {
            return Err(EdgeCommonsError::ReservedTopic(format!(
                "topic '{topic}' targets the reserved UNS class '{}' (state|metric|cfg|log are \
                 library-owned): use the library publishers instead (heartbeat/state keepalive, \
                 the metric subsystem via gg.metrics(), the effective-config publisher)",
                cls.token()
            )));
        }
        Ok(())
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
                .map_err(|_| EdgeCommonsError::Messaging("subscription map poisoned".to_string()))?;
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
                .map_err(|_| EdgeCommonsError::Messaging("subscription map poisoned".to_string()))?;
            map.remove(&(dest, filter.to_string()))
        };
        if let Some(task) = task {
            task.abort();
        }
        self.provider.unsubscribe(filter, dest).await
    }

    /// Resolve the deadline for one `request()` call: an explicit per-call timeout
    /// wins (`Some(ZERO)` = disabled for that call); `None` falls back to the
    /// service default (§5/D-U5).
    fn effective_request_timeout(&self, per_call: Option<Duration>) -> Option<Duration> {
        match per_call {
            Some(d) if d.is_zero() => None,
            Some(d) => Some(d),
            None => self.default_request_timeout(),
        }
    }

    /// Issue a request on `dest` and return its reply handle.
    ///
    /// **The §5 supervisor**: a spawned task OWNS the ephemeral reply subscription
    /// and `select!`s over reply-arrival / the framework deadline / cancel. The
    /// select is the **single idempotent settle site** — exactly one arm wins; the
    /// task then unsubscribes the reply topic (on the correct destination) and
    /// sends the outcome down a oneshot. Cleanup therefore runs on every settle
    /// path even when the returned [`ReplyFuture`] is never polled (the historical
    /// stored-but-never-polled leak), and a straggler reply after settle is dropped
    /// by the closed channel (logged at debug by the provider router).
    async fn start_request(
        &self,
        topic: &str,
        msg: Message,
        dest: Destination,
        qos: Qos,
        timeout: Option<Duration>,
    ) -> Result<ReplyFuture> {
        let reply_topic = request_reply::new_reply_topic();
        let mut sub = self
            .provider
            .subscribe(&reply_topic, dest, qos, REPLY_QUEUE_SIZE)
            .await?;

        let mut request = msg;
        request.header.reply_to = Some(reply_topic.clone());
        let payload = match request.to_vec() {
            Ok(p) => p,
            Err(e) => {
                // Failed before send: tear the reply subscription back down.
                drop(sub);
                let _ = self.provider.unsubscribe(&reply_topic, dest).await;
                return Err(e);
            }
        };
        if let Err(e) = self.provider.publish(topic, payload, dest, qos).await {
            drop(sub);
            let _ = self.provider.unsubscribe(&reply_topic, dest).await;
            return Err(e);
        }

        let effective = self.effective_request_timeout(timeout);
        let (result_tx, result_rx) = oneshot::channel::<Result<Message>>();
        let (cancel_tx, mut cancel_rx) = oneshot::channel::<()>();
        let provider = self.provider.clone();
        let request_topic = topic.to_string();

        tokio::spawn(async move {
            // The deadline arm: sleeps for the effective timeout, or pends forever
            // when the deadline is disabled.
            let deadline = async {
                match effective {
                    Some(d) => tokio::time::sleep(d).await,
                    None => std::future::pending::<()>().await,
                }
            };
            // Single idempotent settle: exactly one select arm wins.
            let outcome: Result<Message> = tokio::select! {
                reply = sub.recv() => match reply {
                    Some((_topic, bytes)) => Message::from_slice(&bytes),
                    None => Err(EdgeCommonsError::Messaging(
                        "reply channel closed before a reply arrived".to_string(),
                    )),
                },
                _ = deadline => Err(EdgeCommonsError::RequestTimeout {
                    topic: request_topic.clone(),
                    secs: effective.map(|d| d.as_secs_f64()).unwrap_or(0.0),
                }),
                // Resolves on explicit cancel AND on ReplyFuture drop (sender dropped).
                _ = &mut cancel_rx => Err(EdgeCommonsError::Messaging(format!(
                    "request on '{request_topic}' was cancelled before a reply arrived"
                ))),
            };
            if matches!(outcome, Err(EdgeCommonsError::RequestTimeout { .. })) {
                tracing::warn!(
                    topic = %request_topic,
                    reply_topic = %reply_topic,
                    "request deadline fired; cleaning up the reply subscription"
                );
            }
            // Cleanup BEFORE settling the caller: drop the local routing entry, then
            // UNSUBSCRIBE at the broker on the SAME destination the request used.
            drop(sub);
            let _ = provider.unsubscribe(&reply_topic, dest).await;
            // The caller may be gone (future dropped) — a failed send is fine.
            let _ = result_tx.send(outcome);
        });

        Ok(ReplyFuture { rx: result_rx, cancel: Some(cancel_tx) })
    }

    /// Publish a reply correlated with `request` on `dest`.
    async fn send_reply(&self, request: &Message, reply: Message, dest: Destination) -> Result<()> {
        let topic = request.header.reply_to.clone().ok_or_else(|| {
            EdgeCommonsError::Messaging("cannot reply: request has no reply_to".to_string())
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
impl ReservedMessaging for DefaultMessagingService {
    /// §4.2: the privileged local publish — bypasses the reserved-class guard.
    async fn publish_reserved(&self, topic: &str, msg: &Message) -> Result<()> {
        self.provider
            .publish(topic, msg.to_vec()?, Destination::Local, LOCAL_QOS)
            .await
    }

    /// §4.2: the privileged IoT Core publish — bypasses the reserved-class guard.
    async fn publish_reserved_to_iot_core(
        &self,
        topic: &str,
        msg: &Message,
        qos: Qos,
    ) -> Result<()> {
        self.provider
            .publish(topic, msg.to_vec()?, Destination::IotCore, qos)
            .await
    }
}

#[async_trait]
impl MessagingService for DefaultMessagingService {
    async fn publish(&self, topic: &str, msg: &Message) -> Result<()> {
        self.check_reserved(Some(topic))?;
        self.provider
            .publish(topic, msg.to_vec()?, Destination::Local, LOCAL_QOS)
            .await
    }

    async fn publish_to_iot_core(&self, topic: &str, msg: &Message, qos: Qos) -> Result<()> {
        self.check_reserved(Some(topic))?;
        self.provider
            .publish(topic, msg.to_vec()?, Destination::IotCore, qos)
            .await
    }

    async fn publish_raw(&self, topic: &str, payload: &Value) -> Result<()> {
        self.check_reserved(Some(topic))?;
        self.provider
            .publish(topic, serde_json::to_vec(payload)?, Destination::Local, LOCAL_QOS)
            .await
    }

    async fn publish_to_iot_core_raw(&self, topic: &str, payload: &Value, qos: Qos) -> Result<()> {
        self.check_reserved(Some(topic))?;
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
        self.request_with_timeout(topic, msg, None).await
    }

    async fn request_from_iot_core(&self, topic: &str, msg: Message) -> Result<ReplyFuture> {
        self.request_from_iot_core_with_timeout(topic, msg, None).await
    }

    async fn request_with_timeout(
        &self,
        topic: &str,
        msg: Message,
        timeout: Option<Duration>,
    ) -> Result<ReplyFuture> {
        self.check_reserved(Some(topic))?;
        self.start_request(topic, msg, Destination::Local, LOCAL_QOS, timeout)
            .await
    }

    async fn request_from_iot_core_with_timeout(
        &self,
        topic: &str,
        msg: Message,
        timeout: Option<Duration>,
    ) -> Result<ReplyFuture> {
        self.check_reserved(Some(topic))?;
        self.start_request(topic, msg, Destination::IotCore, Qos::AtLeastOnce, timeout)
            .await
    }

    async fn reply(&self, request: &Message, reply: Message) -> Result<()> {
        self.check_reserved(request.header.reply_to.as_deref())?;
        self.send_reply(request, reply, Destination::Local).await
    }

    async fn reply_to_iot_core(&self, request: &Message, reply: Message) -> Result<()> {
        self.check_reserved(request.header.reply_to.as_deref())?;
        self.send_reply(request, reply, Destination::IotCore).await
    }

    fn cancel_request(&self, reply_future: ReplyFuture) {
        drop(reply_future); // Drop signals the supervisor's cancel arm.
    }

    fn cancel_request_from_iot_core(&self, reply_future: ReplyFuture) {
        drop(reply_future);
    }

    fn connected(&self) -> bool {
        self.provider.connected()
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
        /// Destinations of each unsubscribe call (wrong-side-unsubscribe guard).
        unsubscribed_dests: Mutex<Vec<Destination>>,
        /// `(topic, payload)` of each publish.
        published: Mutex<Vec<RawMessage>>,
    }

    impl FakeProvider {
        fn new() -> Self {
            Self {
                sender: Mutex::new(None),
                unsubscribed: AtomicUsize::new(0),
                unsubscribed_dests: Mutex::new(Vec::new()),
                published: Mutex::new(Vec::new()),
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
        async fn publish(&self, t: &str, p: Vec<u8>, _d: Destination, _q: Qos) -> Result<()> {
            self.published.lock().unwrap().push((t.to_string(), p));
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
        async fn unsubscribe(&self, _f: &str, d: Destination) -> Result<()> {
            self.unsubscribed.fetch_add(1, Ordering::SeqCst);
            self.unsubscribed_dests.lock().unwrap().push(d);
            Ok(())
        }
        fn connected(&self) -> bool {
            true
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

        // The supervisor unsubscribes the reply topic on settle (no orphan).
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
    async fn dropping_the_reply_future_unsubscribes_reply_topic() {
        // Preserves today's Drop-cleans-up contract (e.g. a caller-side
        // tokio::time::timeout dropping the future).
        let provider = Arc::new(FakeProvider::new());
        let svc = DefaultMessagingService::new(provider.clone());

        let reply_future = svc.request("svc/op", msg(1)).await.unwrap();
        let result = tokio::time::timeout(Duration::from_millis(50), reply_future).await;
        assert!(result.is_err(), "expected the caller-side await to time out");

        wait_for(&provider.unsubscribed, 1).await;
    }

    // ---------- §5 framework-owned request deadline ----------

    fn timeout_code(err: EdgeCommonsError) -> (String, f64) {
        match err {
            EdgeCommonsError::RequestTimeout { topic, secs } => (topic, secs),
            other => panic!("expected RequestTimeout, got {other}"),
        }
    }

    #[tokio::test]
    async fn deadline_fires_even_if_the_future_is_never_polled() {
        let provider = Arc::new(FakeProvider::new());
        let svc = DefaultMessagingService::new(provider.clone());

        // Short explicit deadline; store the future WITHOUT polling it.
        let reply_future = svc
            .request_with_timeout("svc/op", msg(1), Some(Duration::from_millis(50)))
            .await
            .unwrap();

        // The supervisor cleans up on the deadline with zero polls of the future.
        wait_for(&provider.unsubscribed, 1).await;

        // Polling afterwards yields the timeout error immediately.
        let (topic, secs) = timeout_code(reply_future.await.unwrap_err());
        assert_eq!(topic, "svc/op");
        assert!((secs - 0.05).abs() < 1e-9);
    }

    #[tokio::test]
    async fn deadline_resolves_an_awaited_future_with_request_timeout() {
        let provider = Arc::new(FakeProvider::new());
        let svc = DefaultMessagingService::new(provider.clone());
        svc.set_default_request_timeout(Some(Duration::from_millis(40)));

        let reply_future = svc.request("svc/op", msg(1)).await.unwrap();
        let err = reply_future.await.unwrap_err();
        assert!(matches!(err, EdgeCommonsError::RequestTimeout { .. }), "got {err}");
        wait_for(&provider.unsubscribed, 1).await;
    }

    #[tokio::test]
    async fn per_call_zero_disables_the_deadline() {
        let provider = Arc::new(FakeProvider::new());
        let svc = DefaultMessagingService::new(provider.clone());
        svc.set_default_request_timeout(Some(Duration::from_millis(30)));

        let reply_future = svc
            .request_with_timeout("svc/op", msg(1), Some(Duration::ZERO))
            .await
            .unwrap();
        // Well past the default deadline the request is still pending (no unsubscribe).
        tokio::time::sleep(Duration::from_millis(120)).await;
        assert_eq!(provider.unsubscribed.load(Ordering::SeqCst), 0);

        // A late reply still completes it.
        provider.push("reply", &msg(7));
        let reply = reply_future.await.unwrap();
        assert_eq!(reply.body.as_u64().unwrap(), 7);
        wait_for(&provider.unsubscribed, 1).await;
    }

    #[tokio::test]
    async fn reply_beats_deadline() {
        let provider = Arc::new(FakeProvider::new());
        let svc = DefaultMessagingService::new(provider.clone());

        let reply_future = svc
            .request_with_timeout("svc/op", msg(1), Some(Duration::from_secs(5)))
            .await
            .unwrap();
        provider.push("reply", &msg(42));
        let reply = reply_future.await.unwrap();
        assert_eq!(reply.body.as_u64().unwrap(), 42);
        // Exactly one settle => exactly one unsubscribe.
        wait_for(&provider.unsubscribed, 1).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(provider.unsubscribed.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn default_timeout_getter_and_setter() {
        let provider = Arc::new(FakeProvider::new());
        let svc = DefaultMessagingService::new(provider);
        // Built-in 30 s until late-bound (so the bootstrap request has a deadline).
        assert_eq!(svc.default_request_timeout(), Some(Duration::from_secs(30)));
        svc.set_default_request_timeout(Some(Duration::from_secs(5)));
        assert_eq!(svc.default_request_timeout(), Some(Duration::from_secs(5)));
        svc.set_default_request_timeout(None);
        assert_eq!(svc.default_request_timeout(), None);
        svc.set_default_request_timeout(Some(Duration::ZERO));
        assert_eq!(svc.default_request_timeout(), None, "zero disables");
    }

    #[tokio::test]
    async fn request_unsubscribes_on_the_request_destination() {
        // Wrong-side-unsubscribe guard: an IoT Core request must clean up its reply
        // subscription on the IoT Core side, not the local side.
        let provider = Arc::new(FakeProvider::new());
        let svc = DefaultMessagingService::new(provider.clone());
        let reply_future = svc
            .request_from_iot_core_with_timeout("svc/op", msg(1), Some(Duration::from_millis(30)))
            .await
            .unwrap();
        let _ = reply_future.await;
        wait_for(&provider.unsubscribed, 1).await;
        assert_eq!(
            provider.unsubscribed_dests.lock().unwrap().as_slice(),
            &[Destination::IotCore]
        );
    }

    // ---------- §4.1 reserved-class publish guard ----------

    fn assert_reserved(err: EdgeCommonsError) {
        assert!(matches!(err, EdgeCommonsError::ReservedTopic(_)), "expected ReservedTopic, got {err}");
    }

    #[tokio::test]
    async fn guard_rejects_reserved_topics_on_every_publish_path() {
        let provider = Arc::new(FakeProvider::new());
        let svc = DefaultMessagingService::new(provider.clone());
        let reserved = "ecv1/gw-01/comp/main/state";

        assert_reserved(svc.publish(reserved, &msg(1)).await.unwrap_err());
        assert_reserved(
            svc.publish_to_iot_core(reserved, &msg(1), Qos::AtLeastOnce).await.unwrap_err(),
        );
        assert_reserved(svc.publish_raw(reserved, &json!({})).await.unwrap_err());
        assert_reserved(
            svc.publish_to_iot_core_raw(reserved, &json!({}), Qos::AtLeastOnce)
                .await
                .unwrap_err(),
        );
        assert_reserved(svc.request(reserved, msg(1)).await.err().unwrap());
        assert_reserved(svc.request_from_iot_core(reserved, msg(1)).await.err().unwrap());
        assert!(provider.published.lock().unwrap().is_empty(), "nothing reached the provider");
    }

    #[tokio::test]
    async fn guard_rejects_hostile_reply_to() {
        // D-U8: a hostile requester setting reply_to to a reserved topic must not
        // turn an innocent responder into a forger.
        let provider = Arc::new(FakeProvider::new());
        let svc = DefaultMessagingService::new(provider.clone());
        let request = MessageBuilder::new("Req", "1.0")
            .reply_to("ecv1/victim/comp/main/cfg")
            .build();
        assert_reserved(svc.reply(&request, msg(1)).await.unwrap_err());
        assert_reserved(svc.reply_to_iot_core(&request, msg(1)).await.unwrap_err());
        assert!(provider.published.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn guard_allows_app_topics_and_non_uns_topics() {
        let provider = Arc::new(FakeProvider::new());
        let svc = DefaultMessagingService::new(provider.clone());
        svc.publish("ecv1/gw-01/comp/main/data/temp", &msg(1)).await.unwrap();
        svc.publish("ecv1/gw-01/comp/main/app/state", &msg(1)).await.unwrap();
        svc.publish("edgecommons/reply-42", &msg(1)).await.unwrap();
        svc.publish("cloudwatch/metric/put", &msg(1)).await.unwrap();
        assert_eq!(provider.published.lock().unwrap().len(), 4);
    }

    #[tokio::test]
    async fn guard_position_five_applies_only_when_root_bound() {
        let provider = Arc::new(FakeProvider::new());
        let svc = DefaultMessagingService::new(provider.clone());
        let rooted_state = "ecv1/dallas/gw-01/comp/main/state";
        // Pre-bind (rootless): position 5 is NOT checked.
        svc.publish(rooted_state, &msg(1)).await.unwrap();
        // Bound to effective root: position 5 IS checked.
        svc.set_guard_include_root(true);
        assert_reserved(svc.publish(rooted_state, &msg(1)).await.unwrap_err());
        // Position 4 stays checked either way.
        assert_reserved(svc.publish("ecv1/d/c/i/metric/x", &msg(1)).await.unwrap_err());
    }

    #[tokio::test]
    async fn reserved_seam_bypasses_the_guard() {
        // §4.2: the crate-private seam is how the library's own publishers reach
        // the reserved classes.
        let provider = Arc::new(FakeProvider::new());
        let svc = DefaultMessagingService::new(provider.clone());
        let reserved: &dyn ReservedMessaging = &svc;
        reserved.publish_reserved("ecv1/gw-01/comp/main/state", &msg(1)).await.unwrap();
        reserved
            .publish_reserved_to_iot_core("ecv1/gw-01/comp/main/metric/sys", &msg(1), Qos::AtLeastOnce)
            .await
            .unwrap();
        assert_eq!(provider.published.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn subscribe_is_never_guarded() {
        let provider = Arc::new(FakeProvider::new());
        let svc = DefaultMessagingService::new(provider.clone());
        // Consumers must be able to read reserved classes.
        svc.subscribe(
            "ecv1/+/+/+/state",
            message_handler(|_t, _m| async {}),
            4,
            1,
        )
        .await
        .unwrap();
    }
}
