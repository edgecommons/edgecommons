//! # Messaging — MQTT provider (standalone)
//!
//! **One-liner purpose**: A [`MessagingProvider`] backed by `rumqttc`, managing a
//! local broker connection (and, in a later sub-step, AWS IoT Core over TLS).
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
//! - **Scope**: this sub-step implements the **local broker over plain TCP**.
//!   Configuring `iotCore` returns a clear error (TLS lands next) rather than
//!   connecting insecurely.
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
use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS};
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::error::{GgError, Result};
use crate::messaging::config::{BrokerConfig, MessagingConfig};
use crate::messaging::{topic_matches, Destination, MessagingProvider, Qos, Subscription};

/// How long [`MqttProvider::connect`] waits for the first `CONNACK`.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Event-loop request channel capacity.
const EVENTLOOP_CAP: usize = 32;

/// One subscription's routing entry.
struct SubEntry {
    filter: String,
    qos: QoS,
    sender: UnboundedSender<(String, Vec<u8>)>,
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

/// MQTT [`MessagingProvider`] over one or more broker connections.
pub struct MqttProvider {
    local: BrokerConn,
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
    /// | `GgError::Config` | Local broker host missing, or `iotCore` configured (TLS not yet supported) | Fix the messaging config |
    /// | `GgError::Messaging` | CONNACK not received within the connect timeout | Verify the broker is up and reachable |
    pub async fn connect(config: &MessagingConfig) -> Result<MqttProvider> {
        if config.messaging.iot_core.is_some() {
            return Err(GgError::Config(
                "IoT Core (TLS) messaging is not implemented yet; configure only 'local' for now"
                    .to_string(),
            ));
        }
        let local = connect_broker(&config.messaging.local).await?;
        Ok(MqttProvider { local })
    }

    /// Resolve the broker connection for a destination.
    fn conn(&self, dest: Destination) -> Result<&BrokerConn> {
        match dest {
            Destination::Local => Ok(&self.local),
            Destination::IotCore => Err(GgError::Messaging(
                "IoT Core destination is not available (TLS not yet implemented)".to_string(),
            )),
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

    async fn subscribe(&self, filter: &str, dest: Destination, qos: Qos) -> Result<Subscription> {
        let conn = self.conn(dest)?;
        let rqos = to_rumqttc_qos(qos);
        let (tx, rx) = mpsc::unbounded_channel();
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
async fn connect_broker(broker: &BrokerConfig) -> Result<BrokerConn> {
    let host = broker.resolved_host()?.to_string();
    let mut options = MqttOptions::new(broker.client_id.clone(), host, broker.port);
    options.set_keep_alive(Duration::from_secs(30));
    options.set_clean_session(true);
    if let Some(creds) = &broker.credentials {
        if let (Some(u), Some(p)) = (&creds.username, &creds.password) {
            options.set_credentials(u.clone(), p.clone());
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
                                let _ = entry.sender.send((p.topic.clone(), p.payload.to_vec()));
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
fn to_rumqttc_qos(qos: Qos) -> QoS {
    match qos {
        Qos::AtMostOnce => QoS::AtMostOnce,
        Qos::AtLeastOnce => QoS::AtLeastOnce,
    }
}
