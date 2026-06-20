//! # Messaging — MQTT provider (standalone)
//!
//! **One-liner purpose**: A [`MessagingProvider`] backed by `rumqttc`, managing a
//! local broker connection and an optional AWS IoT Core connection, both supporting TLS.
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
//! - **Transport/TLS**: the local broker uses plain TCP, server TLS (CA only), or
//!   mutual TLS (CA + client cert/key) depending on its `credentials`; AWS IoT Core
//!   always uses mutual TLS (CA + cert + key, all required). Missing/unreadable
//!   credential files are a hard error — there is no insecure fallback.
//! - Error handling: [`crate::error::Result`]; transport failures are logged and
//!   retried by the event loop, never `panic`.
//!
//! ## Usage Example
//! ```no_run
//! # async fn demo(cfg: &ggcommons::config::model::Config) -> ggcommons::Result<()> {
//! use ggcommons::messaging::config::MessagingConfig;
//! use ggcommons::messaging::provider::mqtt::MqttProvider;
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

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS, TlsConfiguration, Transport};
use tokio::sync::mpsc::{self, error::TrySendError, Sender};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::error::{GgError, Result};
use crate::messaging::config::{BrokerConfig, Credentials, MessagingConfig};
use crate::messaging::{topic_matches, Destination, MessagingProvider, Qos, Subscription};

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
    next_id: AtomicU64,
    task: JoinHandle<()>,
}

impl Drop for BrokerConn {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Whether a broker connection is the local broker or AWS IoT Core (which mandates mTLS).
#[derive(Debug, Clone, Copy)]
enum BrokerRole {
    Local,
    IotCore,
}

/// MQTT [`MessagingProvider`] over one or more broker connections.
pub struct MqttProvider {
    local: BrokerConn,
    iot_core: Option<BrokerConn>,
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
    /// | `GgError::Config` | Missing host, or missing/unreadable TLS credentials (IoT Core requires caPath/certPath/keyPath) | Fix the messaging config / credential paths |
    /// | `GgError::Messaging` | CONNACK not received within the connect timeout | Verify the broker is up and reachable |
    ///
    /// The local broker connects over plain TCP unless its credentials include a
    /// `caPath` (then TLS). AWS IoT Core always connects over mutual TLS
    /// (caPath + certPath + keyPath, all required). Missing/unreadable credential
    /// files are a hard error — there is no insecure fallback.
    pub async fn connect(config: &MessagingConfig) -> Result<MqttProvider> {
        let local = connect_broker(&config.messaging.local, BrokerRole::Local).await?;
        let iot_core = match &config.messaging.iot_core {
            Some(broker) => Some(connect_broker(broker, BrokerRole::IotCore).await?),
            None => None,
        };
        Ok(MqttProvider { local, iot_core })
    }

    /// Resolve the broker connection for a destination.
    fn conn(&self, dest: Destination) -> Result<&BrokerConn> {
        match dest {
            Destination::Local => Ok(&self.local),
            Destination::IotCore => self.iot_core.as_ref().ok_or_else(|| {
                GgError::Messaging("IoT Core destination is not configured".to_string())
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
            .map_err(|e| GgError::Messaging(format!("publish to '{topic}' failed: {e}")))
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
            let mut map = conn
                .registry
                .lock()
                .map_err(|_| GgError::Messaging("subscription registry poisoned".to_string()))?;
            map.insert(
                id,
                SubEntry {
                    filter: filter.to_string(),
                    qos: rqos,
                    sender: tx,
                },
            );
        }

        conn.client
            .subscribe(filter, rqos)
            .await
            .map_err(|e| GgError::Messaging(format!("subscribe to '{filter}' failed: {e}")))?;

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
        conn.client
            .unsubscribe(filter)
            .await
            .map_err(|e| GgError::Messaging(format!("unsubscribe from '{filter}' failed: {e}")))
    }
}

/// Establish one broker connection over plain TCP and block until its first CONNACK.
async fn connect_broker(broker: &BrokerConfig, role: BrokerRole) -> Result<BrokerConn> {
    let host = broker.resolved_host()?.to_string();
    let mut options = MqttOptions::new(broker.client_id.clone(), host.clone(), broker.port);
    options.set_keep_alive(Duration::from_secs(30));
    options.set_clean_session(true);

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

    let (connected_tx, connected_rx) = watch::channel(false);
    let registry_task = registry.clone();
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
    let connected = tokio::time::timeout(CONNECT_TIMEOUT, ready.wait_for(|&c| c)).await;
    match connected {
        Ok(Ok(_)) => Ok(BrokerConn {
            client,
            registry,
            next_id: AtomicU64::new(0),
            task,
        }),
        _ => {
            task.abort();
            Err(GgError::Messaging(format!(
                "timed out waiting {}s for broker CONNACK",
                CONNECT_TIMEOUT.as_secs()
            )))
        }
    }
}

/// Map the library QoS to the `rumqttc` QoS.
/// Build the TLS configuration for a broker, or `None` for a plain connection.
///
/// # Algorithmic Choices
/// - **IoT Core** always uses mutual TLS: `caPath`, `certPath`, and `keyPath` are all
///   required; any missing/unreadable file is a hard error (never an insecure fallback,
///   unlike the Java implementation which silently connected without credentials).
/// - **Local** uses TLS only when `caPath` is set (with optional client auth via
///   `certPath`/`keyPath`); otherwise it connects plain.
fn build_tls(creds: Option<&Credentials>, role: BrokerRole) -> Result<Option<TlsConfiguration>> {
    match role {
        BrokerRole::IotCore => {
            let creds = creds.ok_or_else(|| {
                GgError::Config(
                    "IoT Core requires TLS credentials (caPath, certPath, keyPath)".to_string(),
                )
            })?;
            let ca = read_credential_file(creds.ca_path.as_deref(), "caPath")?;
            let cert = read_credential_file(creds.cert_path.as_deref(), "certPath")?;
            let key = read_credential_file(creds.key_path.as_deref(), "keyPath")?;
            Ok(Some(TlsConfiguration::Simple {
                ca,
                alpn: None,
                client_auth: Some((cert, key)),
            }))
        }
        BrokerRole::Local => {
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
        GgError::Config(format!("TLS requires '{field}' in the messaging credentials"))
    })?;
    std::fs::read(path)
        .map_err(|e| GgError::Config(format!("failed to read {field} '{path}': {e}")))
}

fn to_rumqttc_qos(qos: Qos) -> QoS {
    match qos {
        Qos::AtMostOnce => QoS::AtMostOnce,
        Qos::AtLeastOnce => QoS::AtLeastOnce,
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
            Some(TlsConfiguration::Simple { ca: ca_bytes, client_auth, .. }) => {
                assert_eq!(ca_bytes, b"ca-bytes");
                assert!(client_auth.is_none(), "ca-only => server TLS, no client auth");
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
            Some(TlsConfiguration::Simple { client_auth: Some((cert_b, key_b)), .. }) => {
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
    fn iot_core_requires_ca_cert_and_key() {
        // No credentials at all.
        assert!(build_tls(None, BrokerRole::IotCore).is_err());
        // Missing key => error (no insecure fallback).
        let (ca, cert) = (temp_pem("ca"), temp_pem("cert"));
        let c = creds(Some(&ca), Some(&cert), None);
        assert!(build_tls(Some(&c), BrokerRole::IotCore).is_err());
        for p in [ca, cert] {
            let _ = std::fs::remove_file(p);
        }
    }

    #[test]
    fn iot_core_with_all_three_is_mutual_tls() {
        let (ca, cert, key) = (temp_pem("ca"), temp_pem("cert"), temp_pem("key"));
        let c = creds(Some(&ca), Some(&cert), Some(&key));
        match build_tls(Some(&c), BrokerRole::IotCore).unwrap() {
            Some(TlsConfiguration::Simple { client_auth: Some(_), .. }) => {}
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
}
