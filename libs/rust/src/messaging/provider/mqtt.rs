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
    AsyncClient, Event, LastWill, MqttOptions, Packet, QoS, TlsConfiguration, Transport,
};
use tokio::sync::mpsc::{self, Sender, error::TrySendError};
use tokio::sync::{oneshot, watch};
use tokio::task::JoinHandle;

use crate::error::{EdgeCommonsError, Result};
use crate::messaging::config::{BrokerConfig, Credentials, MessagingConfig};
use crate::messaging::{Destination, MessagingProvider, Qos, Subscription, topic_matches};

/// How long [`MqttProvider::connect`] waits for the first `CONNACK`.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Event-loop request channel capacity.
const EVENTLOOP_CAP: usize = 32;

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
type PendingSubacks = Arc<Mutex<VecDeque<oneshot::Sender<()>>>>;

/// Removes a subscription's routing entry when the [`Subscription`] is dropped.
struct SubGuard {
    registry: Registry,
    id: u64,
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
    registry: Registry,
    pending_subacks: PendingSubacks,
    next_id: AtomicU64,
    task: JoinHandle<()>,
    /// Live connection state: the event-loop task sets it `true` on each `CONNACK` and `false`
    /// on a connection error. Read (latest value) by [`MqttProvider::connected`] for `/readyz`.
    connected: watch::Receiver<bool>,
}

impl Drop for BrokerConn {
    fn drop(&mut self) {
        self.task.abort();
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
        conn.client
            .publish(topic, to_rumqttc_qos(qos), false, payload)
            .await
            .map_err(|e| EdgeCommonsError::Messaging(format!("publish to '{topic}' failed: {e}")))
    }

    async fn subscribe(
        &self,
        filter: &str,
        dest: Destination,
        qos: Qos,
        max_messages: usize,
    ) -> Result<Subscription> {
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

        // Register a SUBACK waiter before sending the SUBSCRIBE so the event loop can signal us.
        let (ack_tx, ack_rx) = oneshot::channel();
        {
            let mut q = conn
                .pending_subacks
                .lock()
                .map_err(|_| EdgeCommonsError::Messaging("suback queue poisoned".to_string()))?;
            q.push_back(ack_tx);
        }

        conn.client.subscribe(filter, rqos).await.map_err(|e| {
            EdgeCommonsError::Messaging(format!("subscribe to '{filter}' failed: {e}"))
        })?;

        // Block until the broker confirms the subscription (SUBACK), so a publish issued right
        // after subscribe isn't lost — parity with Java/Python/TS. Fall back to proceeding (the
        // entry is registered and will be re-subscribed on reconnect) if no SUBACK arrives in time.
        match tokio::time::timeout(CONNECT_TIMEOUT, ack_rx).await {
            Ok(Ok(())) => {}
            _ => tracing::warn!(filter, "SUBACK not observed within timeout; proceeding"),
        }

        let guard = SubGuard {
            registry: conn.registry.clone(),
            id,
        };
        Ok(Subscription::new(rx, Box::new(guard)))
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

    let (client, mut eventloop) = AsyncClient::new(options, EVENTLOOP_CAP);
    let registry: Registry = Arc::new(Mutex::new(HashMap::new()));
    let pending_subacks: PendingSubacks = Arc::new(Mutex::new(VecDeque::new()));

    let (connected_tx, connected_rx) = watch::channel(false);
    let registry_task = registry.clone();
    let pending_task = pending_subacks.clone();
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
                Ok(Event::Incoming(Packet::SubAck(_))) => {
                    // Wake the oldest outstanding subscribe() (FIFO; SUBACKs are ordered).
                    if let Ok(mut q) = pending_task.lock() {
                        if let Some(tx) = q.pop_front() {
                            let _ = tx.send(());
                        }
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
            registry,
            pending_subacks,
            next_id: AtomicU64::new(0),
            task,
            connected: ready,
        })
    } else {
        task.abort();
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
}
