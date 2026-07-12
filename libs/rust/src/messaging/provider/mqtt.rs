//! # Messaging — MQTT provider (standalone)
//!
//! **One-liner purpose**: A [`MessagingProvider`] backed by `rumqttc`, managing a
//! local broker connection and an optional generic northbound MQTT connection.
//!
//! ## Overview
//! On connect, the provider spawns a background task that drives the `rumqttc`
//! event loop: it routes incoming publishes to matching subscriptions and
//! re-subscribes every filter on each (re)connection. `connect` blocks until the
//! first `CONNACK` is observed (matching the Java "connections block until
//! confirmed" semantics).
//!
//! ## Semantics & Architecture
//! - **Thread-safety**: the subscription registry is an `Arc<Mutex<…>>`; the lock
//!   is never held across an `.await`.
//! - **Reconnection**: `rumqttc`'s event loop reconnects automatically; the task
//!   re-subscribes all registered filters on every `CONNACK`, so subscriptions
//!   survive reconnects (closing the Java `connectionLost` no-op gap).
//! - **Cleanup (RAII)**: dropping a [`Subscription`] removes its routing entry;
//!   dropping the provider aborts the event-loop task.
//! - **Transport/TLS**: local and northbound brokers use plain TCP, server TLS (CA only), or
//!   mutual TLS (CA + client cert/key) depending on their `credentials`. Missing/unreadable
//!   configured credential files are a hard error.
//! - Error handling: [`crate::error::Result`]; transport failures are logged and
//!   retried by the event loop, never `panic`.
//!
//! ## Usage Example
//! ```no_run
//! # async fn demo(cfg: &edgecommons::config::model::Config) -> edgecommons::Result<()> {
//! use edgecommons::messaging::config::MessagingConfig;
//! use edgecommons::messaging::provider::mqtt::MqttProvider;
//! let mc = MessagingConfig::load("standalone-messaging.json").await?;
//! let provider = MqttProvider::connect(&mc).await?;
//! # let _ = provider; let _ = cfg;
//! # Ok(())
//! # }
//! ```
//!
//! ## Safety & Panics
//! A poisoned registry mutex (only if another holder panicked) propagates as a
//! panic on lock; in normal operation this cannot happen since no panics occur
//! while the lock is held.
//!
//! ## Related Modules
//! - [`crate::messaging`], [`crate::messaging::config`].

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use rumqttc::{
    AsyncClient, Event, LastWill, MqttOptions, Outgoing, Packet, QoS, SubscribeReasonCode,
    TlsConfiguration, Transport,
};
use tokio::sync::mpsc::{self, Sender, error::TrySendError};
use tokio::sync::{Notify, oneshot, watch};
use tokio::task::JoinHandle;

use crate::error::{EdgeCommonsError, Result};
use crate::messaging::config::{BrokerConfig, Credentials, MessagingConfig};
use crate::messaging::{Destination, MessagingProvider, Qos, Subscription, topic_matches};

/// How long [`MqttProvider::connect`] waits for the first `CONNACK`.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Event-loop request channel capacity.
const EVENTLOOP_CAP: usize = 32;
/// Bounded ingress for the single per-connection publish funnel.
const PUBLISH_FUNNEL_CAP: usize = 1024;
/// Maximum publish markers awaiting packet-id assignment or acknowledgement.
const MAX_TRACKED_PUBLISHES: usize = 1024;
/// Minimum MQTT packet budget for EdgeCommons payloads. `rumqttc` defaults to
/// 10 KiB for both directions, which is far below EMQX's 1 MiB default and too
/// small for legitimate command replies such as OPC UA address-space pages.
const MQTT_MAX_PACKET_BYTES: usize = 1024 * 1024;

/// One subscription's routing entry.
struct SubEntry {
    filter: String,
    qos: QoS,
    sender: Sender<(String, Vec<u8>)>,
}

/// Shared subscription registry for a broker connection.
type Registry = Arc<Mutex<HashMap<u64, SubEntry>>>;

/// FIFO of one-shot waiters that `subscribe` blocks on until the matching `SUBACK` arrives.
/// MQTT acks SUBSCRIBEs in order on a single connection, so the front waiter corresponds to the
/// oldest outstanding subscribe. (Reconnect re-subscribes run in the event loop with no waiter
/// pending, so their SUBACKs simply pop an empty queue.)
struct PendingSuback {
    id: u64,
    result: oneshot::Sender<Result<()>>,
}

type PendingSubacks = Arc<Mutex<VecDeque<PendingSuback>>>;

/// One application publish tracked from funnel submission through its transport acknowledgement.
#[derive(Debug)]
struct TrackedPublish {
    id: u64,
    qos: QoS,
    confirmation: Option<oneshot::Sender<Result<()>>>,
}

/// Ordered pre-wire markers plus packet-id-addressable in-flight QoS 1/2 publishes.
///
/// Every publish, including ordinary fire-and-forget QoS publishes, receives a marker. That is
/// what makes the FIFO `Outgoing::Publish(pkid)` event safe to associate under concurrent traffic.
/// Once a nonzero packet id is observed, the marker remains in `inflight` until its PUBACK/PUBCOMP;
/// retransmitted `Outgoing::Publish(pkid)` events therefore cannot consume the next marker.
#[derive(Default)]
struct PublishTracking {
    awaiting_outgoing: VecDeque<TrackedPublish>,
    inflight: HashMap<u16, TrackedPublish>,
}

impl PublishTracking {
    fn len(&self) -> usize {
        self.awaiting_outgoing.len() + self.inflight.len()
    }

    fn register(&mut self, publish: TrackedPublish) -> std::result::Result<(), TrackedPublish> {
        if self.len() >= MAX_TRACKED_PUBLISHES {
            return Err(publish);
        }
        self.awaiting_outgoing.push_back(publish);
        Ok(())
    }

    fn remove(&mut self, id: u64) -> Option<TrackedPublish> {
        if let Some(index) = self
            .awaiting_outgoing
            .iter()
            .position(|publish| publish.id == id)
        {
            return self.awaiting_outgoing.remove(index);
        }
        let pkid = self
            .inflight
            .iter()
            .find_map(|(pkid, publish)| (publish.id == id).then_some(*pkid));
        pkid.and_then(|pkid| self.inflight.remove(&pkid))
    }

    /// Observe a wire send. Returns `true` when a marker left the bounded tracker.
    fn observe_outgoing(&mut self, pkid: u16) -> bool {
        if pkid != 0 && self.inflight.contains_key(&pkid) {
            tracing::debug!(pkid, "observed retransmitted MQTT publish");
            return false;
        }

        let Some(mut publish) = self.awaiting_outgoing.pop_front() else {
            tracing::error!(
                pkid,
                "MQTT emitted an outgoing publish with no funnel marker; confirmation disabled for it"
            );
            return false;
        };

        match (publish.qos, pkid) {
            (QoS::AtMostOnce, 0) => {
                if let Some(tx) = publish.confirmation.take() {
                    let _ = tx.send(Err(EdgeCommonsError::Messaging(
                        "confirmed MQTT publish cannot use QoS 0".to_string(),
                    )));
                }
                true
            }
            (QoS::AtLeastOnce | QoS::ExactlyOnce, 1..) => {
                self.inflight.insert(pkid, publish);
                false
            }
            (qos, packet_id) => {
                if let Some(tx) = publish.confirmation.take() {
                    let _ = tx.send(Err(EdgeCommonsError::Messaging(format!(
                        "MQTT publish tracker observed incompatible QoS {qos:?} and packet id {packet_id}; outcome is ambiguous"
                    ))));
                }
                tracing::error!(
                    publish_id = publish.id,
                    ?qos,
                    packet_id,
                    "MQTT publish tracker lost protocol alignment"
                );
                true
            }
        }
    }

    /// Observe a PUBACK. Returns `true` when a marker left the bounded tracker.
    fn observe_puback(&mut self, pkid: u16) -> bool {
        let Some(mut publish) = self.inflight.remove(&pkid) else {
            tracing::debug!(pkid, "PUBACK had no tracked application waiter");
            return false;
        };
        if publish.qos != QoS::AtLeastOnce {
            tracing::error!(
                pkid,
                qos = ?publish.qos,
                "PUBACK did not match a QoS 1 publish marker"
            );
            if let Some(tx) = publish.confirmation.take() {
                let _ = tx.send(Err(EdgeCommonsError::Messaging(
                    "MQTT PUBACK did not match the tracked QoS; outcome is ambiguous".to_string(),
                )));
            }
            return true;
        }
        if let Some(tx) = publish.confirmation.take() {
            let _ = tx.send(Ok(()));
        }
        true
    }

    /// Observe a PUBCOMP. Returns `true` when a marker left the bounded tracker.
    fn observe_pubcomp(&mut self, pkid: u16) -> bool {
        if let Some(mut publish) = self.inflight.remove(&pkid) {
            if let Some(tx) = publish.confirmation.take() {
                let _ = tx.send(Err(EdgeCommonsError::Messaging(
                    "confirmed MQTT publish requires QoS 1, not QoS 2".to_string(),
                )));
            }
            true
        } else {
            false
        }
    }

    fn fail_confirmations(&mut self, detail: &str) {
        for publish in self
            .awaiting_outgoing
            .iter_mut()
            .chain(self.inflight.values_mut())
        {
            if let Some(tx) = publish.confirmation.take() {
                let _ = tx.send(Err(EdgeCommonsError::Messaging(detail.to_string())));
            }
        }
    }
}

type PublishTracker = Arc<Mutex<PublishTracking>>;

/// One command entering the per-connection publish funnel.
struct PublishCommand {
    id: u64,
    topic: String,
    payload: Vec<u8>,
    qos: QoS,
    confirmation: Option<oneshot::Sender<Result<()>>>,
    submitted: oneshot::Sender<Result<()>>,
}

/// Removes a subscription's routing entry when the [`Subscription`] is dropped.
struct SubGuard {
    registry: Registry,
    id: u64,
}

struct PendingSubackGuard {
    pending: PendingSubacks,
    id: u64,
}

impl Drop for PendingSubackGuard {
    fn drop(&mut self) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.retain(|waiter| waiter.id != self.id);
        }
    }
}

impl Drop for SubGuard {
    fn drop(&mut self) {
        if let Ok(mut map) = self.registry.lock() {
            map.remove(&self.id);
        }
    }
}

/// A single broker connection (client + event-loop task + routing registry).
struct BrokerConn {
    client: AsyncClient,
    publish_tx: mpsc::Sender<PublishCommand>,
    publish_tracker: PublishTracker,
    registry: Registry,
    pending_subacks: PendingSubacks,
    next_id: AtomicU64,
    next_suback_id: AtomicU64,
    next_publish_id: AtomicU64,
    task: JoinHandle<()>,
    publish_task: JoinHandle<()>,
    /// Live connection state: the event-loop task sets it `true` on each `CONNACK` and `false`
    /// on a connection error. Read (latest value) by [`MqttProvider::connected`] for `/readyz`.
    connected: watch::Receiver<bool>,
}

impl Drop for BrokerConn {
    fn drop(&mut self) {
        if let Ok(mut tracker) = self.publish_tracker.lock() {
            tracker.fail_confirmations(
                "MQTT provider stopped before PUBACK; publish outcome is ambiguous",
            );
        }
        self.task.abort();
        self.publish_task.abort();
    }
}

impl BrokerConn {
    async fn submit_publish(
        &self,
        topic: &str,
        payload: Vec<u8>,
        qos: Qos,
        confirmation: Option<oneshot::Sender<Result<()>>>,
    ) -> Result<u64> {
        let id = self.next_publish_id.fetch_add(1, Ordering::Relaxed);
        let (submitted_tx, submitted_rx) = oneshot::channel();
        self.publish_tx
            .send(PublishCommand {
                id,
                topic: topic.to_string(),
                payload,
                qos: to_rumqttc_qos(qos),
                confirmation,
                submitted: submitted_tx,
            })
            .await
            .map_err(|_| {
                EdgeCommonsError::Messaging(format!("publish funnel for '{topic}' is not running"))
            })?;
        submitted_rx.await.map_err(|_| {
            EdgeCommonsError::Messaging(format!(
                "publish funnel for '{topic}' stopped before submission"
            ))
        })??;
        Ok(id)
    }
}

/// Whether a broker connection is the local broker or the optional northbound broker.
#[derive(Debug, Clone, Copy)]
enum BrokerRole {
    Local,
    Northbound,
}

/// MQTT [`MessagingProvider`] over one or more broker connections.
pub struct MqttProvider {
    local: BrokerConn,
    northbound: Option<BrokerConn>,
}

/// Explicit MQTT Last-Will for specialized callers that own a non-generic MQTT
/// connection, such as `uns-bridge`'s site-broker uplink.
///
/// This is deliberately not part of the generic `messaging` configuration schema.
/// Retain is hard-wired to `false`.
#[derive(Debug, Clone)]
pub struct MqttLastWill {
    pub topic: String,
    pub payload: Vec<u8>,
    pub qos: Qos,
}

impl MqttProvider {
    /// Connect to the broker(s) described by `config`, blocking until the local
    /// broker's first `CONNACK`.
    ///
    /// # Semantics & Syntax
    /// - **Signature**: `pub async fn connect(config: &MessagingConfig) -> Result<MqttProvider>`
    ///
    /// # Pre-conditions
    /// The local broker is reachable at the configured host/port.
    ///
    /// # Post-conditions
    /// The returned provider has an established, confirmed local connection and a
    /// running event-loop task.
    ///
    /// # Errors
    /// | Error Variant | Condition | Recovery |
    /// |---------------|-----------|----------|
    /// | `EdgeCommonsError::Config` | Missing host, or missing/unreadable TLS credentials | Fix the messaging config / credential paths |
    /// | `EdgeCommonsError::Messaging` | CONNACK not received within the connect timeout | Verify the broker is up and reachable |
    ///
    /// Each broker connects over plain TCP unless its credentials include a `caPath`
    /// (then TLS). A cert/key pair additionally enables mutual TLS. Missing/unreadable
    /// configured credential files are a hard error.
    pub async fn connect(config: &MessagingConfig) -> Result<MqttProvider> {
        Self::connect_with_last_will(config, None).await
    }

    /// Connect to the broker(s) described by `config`, registering `local_last_will`
    /// on the local connection when supplied.
    ///
    /// This exists for explicit, component-owned MQTT uplinks (currently
    /// `uns-bridge`'s site-broker connection). The generic EdgeCommons
    /// `messaging` config no longer carries LWT.
    pub async fn connect_with_last_will(
        config: &MessagingConfig,
        local_last_will: Option<&MqttLastWill>,
    ) -> Result<MqttProvider> {
        config.messaging.qos_config().validate()?;
        let local =
            connect_broker(&config.messaging.local, BrokerRole::Local, local_last_will).await?;
        let northbound = match &config.messaging.northbound {
            Some(broker) => Some(connect_broker(broker, BrokerRole::Northbound, None).await?),
            None => None,
        };
        Ok(MqttProvider { local, northbound })
    }

    /// Resolve the broker connection for a destination.
    fn conn(&self, dest: Destination) -> Result<&BrokerConn> {
        match dest {
            Destination::Local => Ok(&self.local),
            Destination::Northbound => self.northbound.as_ref().ok_or_else(|| {
                EdgeCommonsError::Messaging(
                    "northbound MQTT destination is not configured".to_string(),
                )
            }),
        }
    }

    async fn subscribe_with_acknowledgement(
        &self,
        filter: &str,
        dest: Destination,
        qos: Qos,
        max_messages: usize,
        timeout: Duration,
    ) -> Result<Subscription> {
        if timeout.is_zero() {
            return Err(EdgeCommonsError::Messaging(
                "acknowledged MQTT subscribe requires a positive timeout".to_string(),
            ));
        }
        let conn = self.conn(dest)?;
        let rqos = to_rumqttc_qos(qos);
        let (tx, rx) = mpsc::channel(max_messages.max(1));
        let id = conn.next_id.fetch_add(1, Ordering::Relaxed);
        {
            let mut map = conn.registry.lock().map_err(|_| {
                EdgeCommonsError::Messaging("subscription registry poisoned".to_string())
            })?;
            map.insert(
                id,
                SubEntry {
                    filter: filter.to_string(),
                    qos: rqos,
                    sender: tx,
                },
            );
        }
        // These two guards make future cancellation safe: a caller-side timeout cannot leave a
        // routing entry or stale FIFO acknowledgement waiter behind.
        let sub_guard = SubGuard {
            registry: conn.registry.clone(),
            id,
        };
        let ack_id = conn.next_suback_id.fetch_add(1, Ordering::Relaxed);
        let (ack_tx, ack_rx) = oneshot::channel();
        {
            let mut pending = conn
                .pending_subacks
                .lock()
                .map_err(|_| EdgeCommonsError::Messaging("suback queue poisoned".to_string()))?;
            pending.push_back(PendingSuback {
                id: ack_id,
                result: ack_tx,
            });
        }
        let _ack_guard = PendingSubackGuard {
            pending: conn.pending_subacks.clone(),
            id: ack_id,
        };

        conn.client.subscribe(filter, rqos).await.map_err(|error| {
            EdgeCommonsError::Messaging(format!("subscribe to '{filter}' failed: {error}"))
        })?;

        match tokio::time::timeout(timeout, ack_rx).await {
            Ok(Ok(Ok(()))) => Ok(Subscription::new(rx, Box::new(sub_guard))),
            Ok(Ok(Err(error))) => {
                let _ = conn.client.unsubscribe(filter).await;
                Err(error)
            }
            Ok(Err(_)) => {
                let _ = conn.client.unsubscribe(filter).await;
                Err(EdgeCommonsError::Messaging(format!(
                    "SUBACK tracker ended before acknowledgement for '{filter}'"
                )))
            }
            Err(_) => {
                let _ = conn.client.unsubscribe(filter).await;
                Err(EdgeCommonsError::Messaging(format!(
                    "timed out after {}s waiting for MQTT SUBACK on '{filter}'",
                    timeout.as_secs_f64()
                )))
            }
        }
    }
}

#[async_trait]
impl MessagingProvider for MqttProvider {
    async fn publish(
        &self,
        topic: &str,
        payload: Vec<u8>,
        dest: Destination,
        qos: Qos,
    ) -> Result<()> {
        let conn = self.conn(dest)?;
        conn.submit_publish(topic, payload, qos, None).await?;
        Ok(())
    }

    async fn publish_confirmed(
        &self,
        topic: &str,
        payload: Vec<u8>,
        dest: Destination,
        qos: Qos,
        timeout: Duration,
    ) -> Result<()> {
        if qos != Qos::AtLeastOnce {
            return Err(EdgeCommonsError::Messaging(
                "confirmed MQTT publish requires QoS 1".to_string(),
            ));
        }
        if timeout.is_zero() {
            return Err(EdgeCommonsError::Messaging(
                "confirmed MQTT publish requires a positive timeout".to_string(),
            ));
        }
        let conn = self.conn(dest)?;
        if !*conn.connected.borrow() {
            return Err(EdgeCommonsError::Messaging(format!(
                "cannot confirm MQTT publish to '{topic}' while disconnected"
            )));
        }

        let (confirmation_tx, confirmation_rx) = oneshot::channel();
        let submit_and_confirm = async {
            conn.submit_publish(topic, payload, Qos::AtLeastOnce, Some(confirmation_tx))
                .await?;
            confirmation_rx.await.map_err(|_| {
                EdgeCommonsError::Messaging(format!(
                    "MQTT confirmation tracker ended before PUBACK for '{topic}'; outcome is ambiguous"
                ))
            })?
        };
        match tokio::time::timeout(timeout, submit_and_confirm).await {
            Ok(result) => result,
            Err(_) => Err(EdgeCommonsError::Messaging(format!(
                "timed out after {}s submitting or waiting for MQTT PUBACK on '{topic}'; outcome is ambiguous",
                timeout.as_secs_f64()
            ))),
        }
    }

    async fn subscribe(
        &self,
        filter: &str,
        dest: Destination,
        qos: Qos,
        max_messages: usize,
    ) -> Result<Subscription> {
        self.subscribe_with_acknowledgement(filter, dest, qos, max_messages, CONNECT_TIMEOUT)
            .await
    }

    async fn subscribe_acknowledged(
        &self,
        filter: &str,
        dest: Destination,
        qos: Qos,
        max_messages: usize,
        timeout: Duration,
    ) -> Result<Subscription> {
        self.subscribe_with_acknowledgement(filter, dest, qos, max_messages, timeout)
            .await
    }

    async fn unsubscribe(&self, filter: &str, dest: Destination) -> Result<()> {
        let conn = self.conn(dest)?;
        if let Ok(mut map) = conn.registry.lock() {
            map.retain(|_, e| e.filter != filter);
        }
        conn.client.unsubscribe(filter).await.map_err(|e| {
            EdgeCommonsError::Messaging(format!("unsubscribe from '{filter}' failed: {e}"))
        })
    }

    fn connected(&self) -> bool {
        // Readiness reflects the LOCAL broker — the connection that serves local traffic. The
        // optional AWS IoT Core link is intermittent by design (cloud cooperation), so it must
        // not gate `/readyz`; an offline cloud keeps the pod ready for local work.
        *self.local.connected.borrow()
    }
}

/// Establish one broker connection and block until its first CONNACK. When a
/// `last_will` is supplied, it is registered on the CONNECT options — rumqttc
/// re-sends the same options on every automatic reconnect, so the will is
/// re-registered for free. The will is registered at CONNECT, **not routed through
/// `publish()`** — the reserved-class guard does not (cannot) apply; broker ACLs
/// govern wills.
async fn connect_broker(
    broker: &BrokerConfig,
    role: BrokerRole,
    last_will: Option<&MqttLastWill>,
) -> Result<BrokerConn> {
    let host = broker.resolved_host()?.to_string();
    let mut options = MqttOptions::new(broker.client_id.clone(), host.clone(), broker.port);
    options.set_keep_alive(Duration::from_secs(30));
    options.set_clean_session(true);
    options.set_max_packet_size(MQTT_MAX_PACKET_BYTES, MQTT_MAX_PACKET_BYTES);
    if let Some(last_will) = last_will {
        options.set_last_will(build_last_will(last_will)?);
    }

    match build_tls(broker.credentials.as_ref(), role)? {
        Some(tls) => {
            options.set_transport(Transport::Tls(tls));
            tracing::info!(host = %host, port = broker.port, "connecting to broker over TLS");
        }
        None => {
            if let Some(creds) = &broker.credentials {
                if let (Some(user), Some(pass)) = (&creds.username, &creds.password) {
                    options.set_credentials(user.clone(), pass.clone());
                }
            }
        }
    }

    tracing::info!(
        host = %host,
        port = broker.port,
        max_packet_bytes = MQTT_MAX_PACKET_BYTES,
        "connecting to broker"
    );

    let (client, mut eventloop) = AsyncClient::new(options, EVENTLOOP_CAP);
    let registry: Registry = Arc::new(Mutex::new(HashMap::new()));
    let pending_subacks: PendingSubacks = Arc::new(Mutex::new(VecDeque::new()));
    let publish_tracker: PublishTracker = Arc::new(Mutex::new(PublishTracking::default()));
    let publish_space = Arc::new(Notify::new());
    let (publish_tx, mut publish_rx) = mpsc::channel::<PublishCommand>(PUBLISH_FUNNEL_CAP);

    let publish_client = client.clone();
    let publish_tracker_task = publish_tracker.clone();
    let publish_space_task = publish_space.clone();
    let publish_task = tokio::spawn(async move {
        'commands: while let Some(command) = publish_rx.recv().await {
            let PublishCommand {
                id,
                topic,
                payload,
                qos,
                confirmation,
                submitted,
            } = command;

            let mut tracked = TrackedPublish {
                id,
                qos,
                confirmation,
            };
            loop {
                let registered = match publish_tracker_task.lock() {
                    Ok(mut tracker) => tracker.register(tracked),
                    Err(_) => {
                        let error = "MQTT publish tracker is poisoned";
                        if let Some(tx) = tracked.confirmation.take() {
                            let _ = tx.send(Err(EdgeCommonsError::Messaging(error.to_string())));
                        }
                        let _ = submitted.send(Err(EdgeCommonsError::Messaging(error.to_string())));
                        continue 'commands;
                    }
                };
                match registered {
                    Ok(()) => break,
                    Err(returned) => {
                        // Preserve ordinary-publish compatibility by exposing bounded
                        // backpressure instead of turning tracker saturation into message loss.
                        tracked = returned;
                        publish_space_task.notified().await;
                    }
                }
            }

            match publish_client.publish(&topic, qos, false, payload).await {
                Ok(()) => {
                    let _ = submitted.send(Ok(()));
                }
                Err(error) => {
                    let tracked = publish_tracker_task
                        .lock()
                        .ok()
                        .and_then(|mut tracker| tracker.remove(id));
                    if tracked.is_some() {
                        publish_space_task.notify_one();
                    }
                    let detail = format!("publish to '{topic}' failed: {error}");
                    if let Some(mut tracked) = tracked {
                        if let Some(tx) = tracked.confirmation.take() {
                            let _ = tx.send(Err(EdgeCommonsError::Messaging(detail.clone())));
                        }
                    }
                    let _ = submitted.send(Err(EdgeCommonsError::Messaging(detail)));
                }
            }
        }
    });

    let (connected_tx, connected_rx) = watch::channel(false);
    let registry_task = registry.clone();
    let pending_task = pending_subacks.clone();
    let publish_tracker_event = publish_tracker.clone();
    let publish_space_event = publish_space.clone();
    let client_task = client.clone();

    let task = tokio::spawn(async move {
        loop {
            match eventloop.poll().await {
                Ok(Event::Incoming(Packet::ConnAck(_))) => {
                    // Re-subscribe every registered filter (covers initial connect and reconnect).
                    let filters: Vec<(String, QoS)> = match registry_task.lock() {
                        Ok(map) => map.values().map(|e| (e.filter.clone(), e.qos)).collect(),
                        Err(_) => Vec::new(),
                    };
                    for (filter, qos) in filters {
                        let _ = client_task.subscribe(filter, qos).await;
                    }
                    let _ = connected_tx.send(true);
                    tracing::info!("MQTT broker connection established");
                }
                Ok(Event::Incoming(Packet::SubAck(ack))) => {
                    // Wake the oldest outstanding subscribe() (FIFO; SUBACKs are ordered).
                    if let Ok(mut q) = pending_task.lock() {
                        if let Some(waiter) = q.pop_front() {
                            let accepted = ack
                                .return_codes
                                .iter()
                                .all(|code| matches!(code, SubscribeReasonCode::Success(_)));
                            let result = if accepted {
                                Ok(())
                            } else {
                                Err(EdgeCommonsError::Messaging(
                                    "MQTT broker rejected subscription in SUBACK".to_string(),
                                ))
                            };
                            let _ = waiter.result.send(result);
                        }
                    }
                }
                Ok(Event::Outgoing(Outgoing::Publish(pkid))) => {
                    let freed = publish_tracker_event
                        .lock()
                        .is_ok_and(|mut tracker| tracker.observe_outgoing(pkid));
                    if freed {
                        publish_space_event.notify_one();
                    }
                }
                Ok(Event::Incoming(Packet::PubAck(ack))) => {
                    let freed = publish_tracker_event
                        .lock()
                        .is_ok_and(|mut tracker| tracker.observe_puback(ack.pkid));
                    if freed {
                        publish_space_event.notify_one();
                    }
                }
                Ok(Event::Incoming(Packet::PubComp(ack))) => {
                    let freed = publish_tracker_event
                        .lock()
                        .is_ok_and(|mut tracker| tracker.observe_pubcomp(ack.pkid));
                    if freed {
                        publish_space_event.notify_one();
                    }
                }
                Ok(Event::Incoming(Packet::Publish(p))) => {
                    if let Ok(map) = registry_task.lock() {
                        for entry in map.values() {
                            if topic_matches(&entry.filter, &p.topic) {
                                // Non-blocking: never stall the shared event loop. A full
                                // queue means the subscription's max_messages is exceeded;
                                // drop with a warning (closed = subscription gone, ignore).
                                match entry.sender.try_send((p.topic.clone(), p.payload.to_vec())) {
                                    Ok(()) => {}
                                    Err(TrySendError::Full(_)) => tracing::warn!(
                                        topic = %p.topic,
                                        filter = %entry.filter,
                                        "subscription queue full (max_messages exceeded); dropping message"
                                    ),
                                    Err(TrySendError::Closed(_)) => {}
                                }
                            }
                        }
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    let _ = connected_tx.send(false);
                    if let Ok(mut tracker) = publish_tracker_event.lock() {
                        tracker.fail_confirmations(
                            "MQTT connection failed before PUBACK; publish outcome is ambiguous",
                        );
                    }
                    tracing::warn!(error = %e, "MQTT connection error; retrying");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    });

    let mut ready = connected_rx;
    let result = tokio::time::timeout(CONNECT_TIMEOUT, ready.wait_for(|&c| c)).await;
    let connacked = matches!(result, Ok(Ok(_)));
    // Release the `Ref` borrow of `ready` before moving it into the BrokerConn.
    drop(result);
    if connacked {
        Ok(BrokerConn {
            client,
            publish_tx,
            publish_tracker,
            registry,
            pending_subacks,
            next_id: AtomicU64::new(0),
            next_suback_id: AtomicU64::new(1),
            next_publish_id: AtomicU64::new(1),
            task,
            publish_task,
            connected: ready,
        })
    } else {
        task.abort();
        publish_task.abort();
        Err(EdgeCommonsError::Messaging(format!(
            "timed out waiting {}s for broker CONNACK",
            CONNECT_TIMEOUT.as_secs()
        )))
    }
}

/// Build the rumqttc [`LastWill`] from an explicit MQTT provider option. Retain is
/// hard-wired to `false`; the caller owns the payload bytes.
///
/// # Errors
/// [`EdgeCommonsError::Config`] on a missing/empty topic or a QoS outside `{0, 1}`.
fn build_last_will(last_will: &MqttLastWill) -> Result<LastWill> {
    if last_will.topic.is_empty() {
        return Err(EdgeCommonsError::Config(
            "MQTT last will topic is required".to_string(),
        ));
    }
    let qos = match last_will.qos {
        Qos::AtMostOnce => QoS::AtMostOnce,
        Qos::AtLeastOnce => QoS::AtLeastOnce,
        Qos::ExactlyOnce => {
            return Err(EdgeCommonsError::Config(format!(
                "MQTT last will QoS must be 0 or 1 (got {})",
                last_will.qos as u8
            )));
        }
    };
    tracing::info!(
        topic = %last_will.topic,
        qos = ?qos,
        "registering MQTT last will (retain=false)"
    );
    Ok(LastWill::new(
        last_will.topic.clone(),
        last_will.payload.clone(),
        qos,
        false,
    ))
}

/// Build the TLS configuration for a broker, or `None` for a plain connection.
///
/// # Algorithmic Choices
/// - Local and northbound use TLS only when `caPath` is set (with optional client auth
///   via `certPath`/`keyPath`); otherwise they connect plain.
fn build_tls(creds: Option<&Credentials>, role: BrokerRole) -> Result<Option<TlsConfiguration>> {
    match role {
        BrokerRole::Local | BrokerRole::Northbound => {
            let Some(creds) = creds else { return Ok(None) };
            let Some(ca_path) = creds.ca_path.as_deref() else {
                return Ok(None); // no CA configured => plain connection
            };
            let ca = read_credential_file(Some(ca_path), "caPath")?;
            let client_auth = match (creds.cert_path.as_deref(), creds.key_path.as_deref()) {
                (Some(cert_path), Some(key_path)) => Some((
                    read_credential_file(Some(cert_path), "certPath")?,
                    read_credential_file(Some(key_path), "keyPath")?,
                )),
                _ => None,
            };
            Ok(Some(TlsConfiguration::Simple {
                ca,
                alpn: None,
                client_auth,
            }))
        }
    }
}

/// Read a required PEM credential file, erroring (never silently skipping) if the
/// path is missing or unreadable.
fn read_credential_file(path: Option<&str>, field: &str) -> Result<Vec<u8>> {
    let path = path.ok_or_else(|| {
        EdgeCommonsError::Config(format!(
            "TLS requires '{field}' in the messaging credentials"
        ))
    })?;
    std::fs::read(path)
        .map_err(|e| EdgeCommonsError::Config(format!("failed to read {field} '{path}': {e}")))
}

fn to_rumqttc_qos(qos: Qos) -> QoS {
    match qos {
        Qos::AtMostOnce => QoS::AtMostOnce,
        Qos::AtLeastOnce => QoS::AtLeastOnce,
        Qos::ExactlyOnce => QoS::ExactlyOnce,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::{Path, PathBuf};

    fn temp_pem(content: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("ggc-pem-{}.pem", uuid::Uuid::new_v4()));
        std::fs::File::create(&path)
            .unwrap()
            .write_all(content.as_bytes())
            .unwrap();
        path
    }

    fn creds(ca: Option<&Path>, cert: Option<&Path>, key: Option<&Path>) -> Credentials {
        let to_s = |p: Option<&Path>| p.map(|p| p.to_string_lossy().into_owned());
        Credentials {
            username: None,
            password: None,
            cert_path: to_s(cert),
            key_path: to_s(key),
            ca_path: to_s(ca),
        }
    }

    #[test]
    fn local_without_ca_is_plain() {
        assert!(build_tls(None, BrokerRole::Local).unwrap().is_none());
        let c = creds(None, None, None);
        assert!(build_tls(Some(&c), BrokerRole::Local).unwrap().is_none());
    }

    #[test]
    fn local_with_ca_only_is_server_tls() {
        let ca = temp_pem("ca-bytes");
        let c = creds(Some(&ca), None, None);
        match build_tls(Some(&c), BrokerRole::Local).unwrap() {
            Some(TlsConfiguration::Simple {
                ca: ca_bytes,
                client_auth,
                ..
            }) => {
                assert_eq!(ca_bytes, b"ca-bytes");
                assert!(
                    client_auth.is_none(),
                    "ca-only => server TLS, no client auth"
                );
            }
            _ => panic!("expected Simple TLS"),
        }
        let _ = std::fs::remove_file(ca);
    }

    #[test]
    fn local_with_ca_cert_key_is_mutual_tls() {
        let (ca, cert, key) = (temp_pem("ca"), temp_pem("cert"), temp_pem("key"));
        let c = creds(Some(&ca), Some(&cert), Some(&key));
        match build_tls(Some(&c), BrokerRole::Local).unwrap() {
            Some(TlsConfiguration::Simple {
                client_auth: Some((cert_b, key_b)),
                ..
            }) => {
                assert_eq!(cert_b, b"cert");
                assert_eq!(key_b, b"key");
            }
            _ => panic!("expected mutual TLS"),
        }
        for p in [ca, cert, key] {
            let _ = std::fs::remove_file(p);
        }
    }

    #[test]
    fn northbound_without_ca_is_plain_mqtt() {
        assert!(build_tls(None, BrokerRole::Northbound).unwrap().is_none());
        let c = Credentials {
            username: Some("u".into()),
            password: Some("p".into()),
            cert_path: None,
            key_path: None,
            ca_path: None,
        };
        assert!(
            build_tls(Some(&c), BrokerRole::Northbound)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn northbound_with_all_three_is_mutual_tls() {
        let (ca, cert, key) = (temp_pem("ca"), temp_pem("cert"), temp_pem("key"));
        let c = creds(Some(&ca), Some(&cert), Some(&key));
        match build_tls(Some(&c), BrokerRole::Northbound).unwrap() {
            Some(TlsConfiguration::Simple {
                client_auth: Some(_),
                ..
            }) => {}
            _ => panic!("expected mutual TLS"),
        }
        for p in [ca, cert, key] {
            let _ = std::fs::remove_file(p);
        }
    }

    #[test]
    fn missing_credential_file_errors_rather_than_falling_back() {
        let c = creds(Some(Path::new("/no/such/ca.pem")), None, None);
        assert!(
            build_tls(Some(&c), BrokerRole::Local).is_err(),
            "an unreadable CA must error, never silently connect"
        );
    }

    #[test]
    fn mqtt_packet_limit_is_at_least_emqx_default() {
        assert_eq!(
            1024 * 1024,
            MQTT_MAX_PACKET_BYTES,
            "EdgeCommons must not inherit rumqttc's 10 KiB packet default"
        );
    }

    // ---------- Explicit MQTT last will provider option ----------

    fn last_will(topic: &str, payload: impl Into<Vec<u8>>, qos: Qos) -> MqttLastWill {
        MqttLastWill {
            topic: topic.to_string(),
            payload: payload.into(),
            qos,
        }
    }

    #[test]
    fn last_will_is_registered_with_retain_false_and_defaults() {
        let will = build_last_will(&last_will(
            "ecv1/gw-01/uns-bridge/main/state",
            br#"{"status":"UNREACHABLE"}"#.to_vec(),
            Qos::AtLeastOnce,
        ))
        .unwrap();
        assert_eq!(will.topic, "ecv1/gw-01/uns-bridge/main/state");
        assert_eq!(will.qos, QoS::AtLeastOnce);
        assert!(
            !will.retain,
            "retain is hard-wired to false (no knob by design)"
        );
        assert_eq!(&will.message[..], br#"{"status":"UNREACHABLE"}"#);
    }

    #[test]
    fn last_will_string_payload_is_verbatim_and_qos_zero_accepted() {
        let will = build_last_will(&last_will("t", b"gone".to_vec(), Qos::AtMostOnce)).unwrap();
        assert_eq!(
            &will.message[..],
            b"gone",
            "caller-owned bytes are published verbatim"
        );
        assert_eq!(will.qos, QoS::AtMostOnce);
    }

    #[test]
    fn last_will_rejects_bad_topic_and_qos() {
        assert!(
            build_last_will(&last_will("", Vec::new(), Qos::AtLeastOnce)).is_err(),
            "empty topic"
        );
        assert!(
            build_last_will(&last_will("t", Vec::new(), Qos::ExactlyOnce)).is_err(),
            "qos 2 is invalid for MQTT last will"
        );
    }

    #[tokio::test]
    async fn ordinary_qos1_publish_cannot_consume_a_later_confirmation() {
        let mut tracker = PublishTracking::default();
        tracker
            .register(TrackedPublish {
                id: 1,
                qos: QoS::AtLeastOnce,
                confirmation: None,
            })
            .unwrap();
        let (confirmed_tx, mut confirmed_rx) = oneshot::channel();
        tracker
            .register(TrackedPublish {
                id: 2,
                qos: QoS::AtLeastOnce,
                confirmation: Some(confirmed_tx),
            })
            .unwrap();

        tracker.observe_outgoing(41);
        tracker.observe_outgoing(42);
        tracker.observe_puback(41);
        assert!(
            tokio::time::timeout(Duration::from_millis(10), &mut confirmed_rx)
                .await
                .is_err(),
            "the ordinary publish's PUBACK must not settle the later confirmed publish"
        );
        tracker.observe_puback(42);
        assert!(confirmed_rx.await.unwrap().is_ok());
    }

    #[tokio::test]
    async fn retransmitted_outgoing_publish_does_not_shift_fifo_alignment() {
        let mut tracker = PublishTracking::default();
        let (first_tx, first_rx) = oneshot::channel();
        tracker
            .register(TrackedPublish {
                id: 1,
                qos: QoS::AtLeastOnce,
                confirmation: Some(first_tx),
            })
            .unwrap();
        tracker.observe_outgoing(7);

        let (second_tx, second_rx) = oneshot::channel();
        tracker
            .register(TrackedPublish {
                id: 2,
                qos: QoS::AtLeastOnce,
                confirmation: Some(second_tx),
            })
            .unwrap();
        tracker.observe_outgoing(7); // reconnect retransmission of the first publish
        tracker.observe_outgoing(8); // the second application's first wire send
        tracker.observe_puback(7);
        tracker.observe_puback(8);

        assert!(first_rx.await.unwrap().is_ok());
        assert!(second_rx.await.unwrap().is_ok());
    }

    #[tokio::test]
    async fn disconnect_fails_confirmation_and_late_puback_cannot_reverse_it() {
        let mut tracker = PublishTracking::default();
        let (confirmed_tx, confirmed_rx) = oneshot::channel();
        tracker
            .register(TrackedPublish {
                id: 1,
                qos: QoS::AtLeastOnce,
                confirmation: Some(confirmed_tx),
            })
            .unwrap();
        tracker.observe_outgoing(9);
        tracker.fail_confirmations(
            "MQTT connection failed before PUBACK; publish outcome is ambiguous",
        );
        tracker.observe_outgoing(9); // retransmit remains a tombstone, not a new marker
        tracker.observe_puback(9);

        assert!(confirmed_rx.await.unwrap().is_err());
        assert_eq!(tracker.len(), 0);
    }

    #[test]
    fn publish_tracker_has_a_hard_1024_entry_bound() {
        let mut tracker = PublishTracking::default();
        for id in 0..MAX_TRACKED_PUBLISHES as u64 {
            tracker
                .register(TrackedPublish {
                    id,
                    qos: QoS::AtMostOnce,
                    confirmation: None,
                })
                .unwrap();
        }
        assert!(
            tracker
                .register(TrackedPublish {
                    id: MAX_TRACKED_PUBLISHES as u64,
                    qos: QoS::AtMostOnce,
                    confirmation: None,
                })
                .is_err()
        );
    }

    /// Register a marker and return its confirmation receiver.
    fn tracked(tracker: &mut PublishTracking, id: u64, qos: QoS) -> oneshot::Receiver<Result<()>> {
        let (tx, rx) = oneshot::channel();
        tracker
            .register(TrackedPublish {
                id,
                qos,
                confirmation: Some(tx),
            })
            .map_err(|_| "tracker full")
            .unwrap();
        rx
    }

    #[tokio::test]
    async fn a_failed_wire_send_untracks_its_marker_so_it_cannot_absorb_a_later_puback() {
        // This is what the publish funnel does when `AsyncClient::publish` errors: it removes
        // the marker it just registered. If `remove` missed the marker, the FIFO would be one
        // entry out of step and the NEXT publish's PUBACK would settle the wrong caller.
        let mut tracker = PublishTracking::default();
        let failed = tracked(&mut tracker, 1, QoS::AtLeastOnce);
        let survivor = tracked(&mut tracker, 2, QoS::AtLeastOnce);

        assert!(
            tracker.remove(1).is_some(),
            "a marker still awaiting the wire must be removable"
        );
        drop(failed);
        assert_eq!(tracker.len(), 1);

        tracker.observe_outgoing(11);
        tracker.observe_puback(11);
        assert!(
            survivor.await.unwrap().is_ok(),
            "the surviving publish must be settled by its own PUBACK"
        );
    }

    #[tokio::test]
    async fn an_inflight_marker_is_removable_by_publish_id() {
        let mut tracker = PublishTracking::default();
        let inflight = tracked(&mut tracker, 7, QoS::AtLeastOnce);
        tracker.observe_outgoing(21); // now addressed by packet id, not by the FIFO

        assert!(
            tracker.remove(7).is_some(),
            "in-flight markers are removable"
        );
        drop(inflight);
        assert_eq!(tracker.len(), 0);
        assert!(
            !tracker.observe_puback(21),
            "a removed marker's PUBACK must free nothing"
        );
        assert!(tracker.remove(7).is_none(), "removal is not repeatable");
    }

    #[tokio::test]
    async fn a_confirmed_publish_at_qos_zero_is_reported_as_unconfirmable() {
        // QoS 0 gets no PUBACK, so a caller awaiting confirmation would hang forever. The
        // tracker must fail it explicitly instead.
        let mut tracker = PublishTracking::default();
        let confirmation = tracked(&mut tracker, 1, QoS::AtMostOnce);

        assert!(
            tracker.observe_outgoing(0),
            "a QoS 0 send leaves the tracker immediately"
        );
        let error = confirmation.await.unwrap().unwrap_err();
        assert!(error.to_string().contains("cannot use QoS 0"), "{error}");
        assert_eq!(tracker.len(), 0);
    }

    #[tokio::test]
    async fn a_qos1_send_without_a_packet_id_is_an_ambiguous_outcome_not_a_success() {
        // Protocol misalignment (a QoS 1/2 marker paired with packet id 0) must never be
        // reported as a confirmed delivery.
        let mut tracker = PublishTracking::default();
        let confirmation = tracked(&mut tracker, 1, QoS::AtLeastOnce);

        assert!(tracker.observe_outgoing(0));
        let error = confirmation.await.unwrap().unwrap_err();
        assert!(error.to_string().contains("ambiguous"), "{error}");
        assert_eq!(tracker.len(), 0);
    }

    #[test]
    fn an_outgoing_publish_with_no_marker_is_survivable() {
        // Nothing to associate: the tracker must not pop a marker belonging to another publish
        // and must not panic.
        let mut tracker = PublishTracking::default();
        assert!(
            !tracker.observe_outgoing(5),
            "no marker left the bounded tracker"
        );
        assert!(
            !tracker.observe_puback(5),
            "a PUBACK for an untracked packet id is ignored"
        );
    }

    #[tokio::test]
    async fn a_puback_that_does_not_match_the_tracked_qos_is_ambiguous() {
        let mut tracker = PublishTracking::default();
        let confirmation = tracked(&mut tracker, 1, QoS::ExactlyOnce);
        tracker.observe_outgoing(31);

        assert!(tracker.observe_puback(31), "the marker leaves the tracker");
        let error = confirmation.await.unwrap().unwrap_err();
        assert!(
            error.to_string().contains("ambiguous"),
            "a QoS 2 publish is not delivered by a PUBACK: {error}"
        );
    }

    #[tokio::test]
    async fn a_pubcomp_settles_the_marker_as_a_confirmation_error() {
        // Confirmed publication is defined at QoS 1. A QoS 2 flow completing with PUBCOMP must
        // release the marker AND tell the caller its confirmation contract was not honored.
        let mut tracker = PublishTracking::default();
        let confirmation = tracked(&mut tracker, 1, QoS::ExactlyOnce);
        tracker.observe_outgoing(41);

        assert!(tracker.observe_pubcomp(41), "the marker leaves the tracker");
        let error = confirmation.await.unwrap().unwrap_err();
        assert!(error.to_string().contains("requires QoS 1"), "{error}");
        assert_eq!(tracker.len(), 0);
        assert!(
            !tracker.observe_pubcomp(41),
            "a duplicate PUBCOMP frees nothing"
        );
    }

    #[test]
    fn every_qos_maps_onto_its_mqtt_counterpart() {
        // The wire QoS is what the broker enforces; a mis-mapping would silently downgrade
        // delivery guarantees.
        assert_eq!(to_rumqttc_qos(Qos::AtMostOnce), QoS::AtMostOnce);
        assert_eq!(to_rumqttc_qos(Qos::AtLeastOnce), QoS::AtLeastOnce);
        assert_eq!(to_rumqttc_qos(Qos::ExactlyOnce), QoS::ExactlyOnce);
    }

    #[test]
    fn a_dropped_subscribe_waiter_leaves_no_stale_fifo_entry() {
        // A caller-side timeout drops the guard; the SUBACK FIFO must not keep the dead waiter,
        // or the NEXT subscribe's SUBACK would be routed to a gone caller.
        let pending: PendingSubacks = Arc::new(Mutex::new(VecDeque::new()));
        let (tx, _rx) = oneshot::channel();
        pending
            .lock()
            .unwrap()
            .push_back(PendingSuback { id: 9, result: tx });
        let guard = PendingSubackGuard {
            pending: pending.clone(),
            id: 9,
        };
        assert_eq!(pending.lock().unwrap().len(), 1);
        drop(guard);
        assert!(
            pending.lock().unwrap().is_empty(),
            "the abandoned waiter must be evicted from the FIFO"
        );
    }
}
