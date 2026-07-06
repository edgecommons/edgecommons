//! # Greengrass IPC runtime
//!
//! **One-liner purpose**: Bridge the synchronous, `no_std`, process-global
//! `aws-greengrass-component-sdk` (lib `gg_sdk`) into this crate's async,
//! `Arc<dyn _>`-based world behind a single shared worker thread.
//!
//! ## Overview
//! The Greengrass component SDK is a thin FFI binding over a C IPC client. Three of
//! its properties make it awkward to use directly from `tokio`:
//! 1. **Synchronous & blocking** — every operation blocks on a Unix-domain-socket
//!    round trip to the Greengrass nucleus.
//! 2. **Process-global** — `Sdk::init()` may be called only once per process, and
//!    the connection is global state.
//! 3. **Lifetime-bound subscriptions** — `subscribe_*` returns a
//!    `Subscription<'a, F>` that borrows the callback `F`; dropping it closes the
//!    subscription at the broker.
//!
//! [`IpcRuntime`] resolves all three by owning the `Sdk` and **every live
//! subscription** on one dedicated OS thread. Callers (the IPC messaging provider
//! and the Greengrass/shadow config sources) interact with it purely through async
//! methods that dispatch a `Command` to the worker and await a `oneshot` reply.
//! Because there is exactly one `Sdk` per process, there is exactly one
//! `IpcRuntime`, obtained via [`global`].
//!
//! ## Semantics & Architecture
//! - **Threading**: a `std::sync::mpsc` command channel feeds the worker; async
//!   methods pair each command with a `tokio::sync::oneshot` reply. Subscription
//!   payloads are delivered on bounded `tokio::sync::mpsc` channels (the same
//!   bounded-queue semantics as the MQTT provider).
//! - **Subscription lifetime**: the worker keeps each SDK `Subscription` alive in a
//!   map keyed by an id; unsubscribing removes it (its `Drop` closes the broker
//!   subscription). The callback closure is `Box::leak`ed to obtain the `'static`
//!   borrow the SDK requires — a small, bounded, one-time-per-subscription leak.
//! - **Config-update re-fetch**: `SubscribeToConfigurationUpdate` delivers only the
//!   changed key path, so on each notification the worker re-fetches the value via
//!   `GetConfiguration` and forwards the fresh JSON document.
//! - **Error handling**: SDK errors are mapped to [`crate::error::EdgeCommonsError::Ipc`];
//!   nothing here panics except the unavoidable one-time `Sdk::init()` contract.
//!
//! ## Status
//! Implemented and **validated on a live Greengrass core** (non-root): IPC connect,
//! local pub/sub, IoT Core bridge (both directions), config fetch + hot reload, and
//! device-shadow get/update. Builds only on Linux (the SDK is a C-FFI crate).
//!
//! ## Related Modules
//! - [`crate::messaging::provider::ipc`], [`crate::config::source::greengrass`],
//!   [`crate::config::source::shadow`].

#![cfg(feature = "greengrass")]

use std::any::Any;
use std::collections::HashMap;
use std::mem::MaybeUninit;
use std::sync::OnceLock;
use std::sync::mpsc::{Receiver as StdReceiver, Sender as StdSender};

use serde_json::Value;
use tokio::sync::{mpsc, oneshot};

use crate::error::{EdgeCommonsError, Result};
use crate::messaging::{Destination, Qos};

/// Buffer size for `GetConfiguration` / `GetThingShadow` decode (bytes).
const IPC_RESULT_BUF: usize = 256 * 1024;

/// A bounded delivery channel for one subscription's `(topic, payload)` messages.
type Delivery = mpsc::Sender<(String, Vec<u8>)>;

/// Commands processed serially by the IPC worker thread.
enum Command {
    /// Connect to the nucleus (idempotent; no-op if already connected).
    Connect(oneshot::Sender<Result<()>>),
    /// Publish raw bytes to a topic on a destination at a QoS.
    Publish {
        topic: String,
        payload: Vec<u8>,
        dest: Destination,
        qos: Qos,
        reply: oneshot::Sender<Result<()>>,
    },
    /// Subscribe to a filter; deliver messages on `out`. Replies with a sub id.
    Subscribe {
        filter: String,
        dest: Destination,
        qos: Qos,
        out: Delivery,
        reply: oneshot::Sender<Result<u64>>,
    },
    /// Drop a subscription by id (best-effort; closes it at the broker).
    Unsubscribe { id: u64 },
    /// Fetch a configuration value as a JSON document.
    GetConfig {
        key_path: Vec<String>,
        component: Option<String>,
        reply: oneshot::Sender<Result<Value>>,
    },
    /// Subscribe to configuration updates; re-fetch and forward on `out`. Replies with a sub id.
    WatchConfig {
        component: Option<String>,
        key_path: Vec<String>,
        out: mpsc::UnboundedSender<Value>,
        reply: oneshot::Sender<Result<u64>>,
    },
    /// Internal: a watched config key changed (carries the watch's sub id).
    ConfigChanged { id: u64 },
    /// Get a thing shadow document (raw bytes).
    GetShadow {
        thing: String,
        shadow: Option<String>,
        reply: oneshot::Sender<Result<Vec<u8>>>,
    },
    /// Update a thing shadow with a raw document.
    UpdateShadow {
        thing: String,
        shadow: Option<String>,
        payload: Vec<u8>,
        reply: oneshot::Sender<Result<()>>,
    },
    /// Delete a thing shadow.
    DeleteShadow {
        thing: String,
        shadow: Option<String>,
        reply: oneshot::Sender<Result<()>>,
    },
}

/// Metadata kept for a config-update watch so the worker can re-fetch on change.
struct ConfigWatch {
    component: Option<String>,
    key_path: Vec<String>,
    out: mpsc::UnboundedSender<Value>,
}

/// A live subscription owned by the worker: the SDK subscription (kept alive so it
/// is not closed) plus, for config watches, the re-fetch metadata.
struct SubEntry {
    /// The SDK `Subscription<'static, F>`, type-erased. Dropping it closes the broker
    /// subscription.
    _sub: Box<dyn Any>,
    config_watch: Option<ConfigWatch>,
}

/// Handle to the single process-wide Greengrass IPC worker.
pub struct IpcRuntime {
    tx: StdSender<Command>,
}

static RUNTIME: OnceLock<IpcRuntime> = OnceLock::new();

/// Get the process-global IPC runtime, spawning its worker thread on first use.
///
/// # Purpose
/// Provide the single shared entry point to Greengrass IPC. Because `Sdk::init()`
/// may run only once per process, there is exactly one runtime.
///
/// # Post-conditions
/// The worker thread is running and the SDK is initialized (but not necessarily
/// connected — call [`IpcRuntime::connect`] before issuing operations).
pub fn global() -> &'static IpcRuntime {
    RUNTIME.get_or_init(IpcRuntime::spawn)
}

impl IpcRuntime {
    /// Spawn the worker thread and return a handle to it.
    fn spawn() -> IpcRuntime {
        let (tx, rx) = std::sync::mpsc::channel::<Command>();
        let internal_tx = tx.clone();
        std::thread::Builder::new()
            .name("edgecommons-ipc".to_string())
            .spawn(move || worker(rx, internal_tx))
            .expect("failed to spawn edgecommons IPC worker thread");
        IpcRuntime { tx }
    }

    /// Dispatch a command that produces a reply and await it.
    async fn call<T>(&self, make: impl FnOnce(oneshot::Sender<Result<T>>) -> Command) -> Result<T> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(make(reply_tx))
            .map_err(|_| EdgeCommonsError::Ipc("IPC worker thread is not running".to_string()))?;
        reply_rx
            .await
            .map_err(|_| EdgeCommonsError::Ipc("IPC worker dropped the reply".to_string()))?
    }

    /// Connect to the Greengrass nucleus (idempotent).
    pub async fn connect(&self) -> Result<()> {
        self.call(Command::Connect).await
    }

    /// Publish raw bytes to `topic` on `dest` at `qos`.
    pub async fn publish(
        &self,
        topic: &str,
        payload: Vec<u8>,
        dest: Destination,
        qos: Qos,
    ) -> Result<()> {
        let topic = topic.to_string();
        self.call(move |reply| Command::Publish {
            topic,
            payload,
            dest,
            qos,
            reply,
        })
        .await
    }

    /// Subscribe to `filter` on `dest`; messages are delivered on `out`. Returns a
    /// subscription id for [`IpcRuntime::unsubscribe`].
    pub async fn subscribe(
        &self,
        filter: &str,
        dest: Destination,
        qos: Qos,
        out: Delivery,
    ) -> Result<u64> {
        let filter = filter.to_string();
        self.call(move |reply| Command::Subscribe {
            filter,
            dest,
            qos,
            out,
            reply,
        })
        .await
    }

    /// Stop a subscription (best-effort; closes it at the broker).
    pub fn unsubscribe(&self, id: u64) {
        let _ = self.tx.send(Command::Unsubscribe { id });
    }

    /// Fetch a configuration value (the whole component config when `key_path` is empty).
    pub async fn get_config(
        &self,
        key_path: Vec<String>,
        component: Option<String>,
    ) -> Result<Value> {
        self.call(move |reply| Command::GetConfig {
            key_path,
            component,
            reply,
        })
        .await
    }

    /// Watch a configuration key path; the re-fetched JSON document is sent on `out`
    /// after each update. Returns a subscription id for [`IpcRuntime::unsubscribe`].
    pub async fn watch_config(
        &self,
        component: Option<String>,
        key_path: Vec<String>,
        out: mpsc::UnboundedSender<Value>,
    ) -> Result<u64> {
        self.call(move |reply| Command::WatchConfig {
            component,
            key_path,
            out,
            reply,
        })
        .await
    }

    /// Get a thing shadow document.
    pub async fn get_shadow(&self, thing: &str, shadow: Option<String>) -> Result<Vec<u8>> {
        let thing = thing.to_string();
        self.call(move |reply| Command::GetShadow {
            thing,
            shadow,
            reply,
        })
        .await
    }

    /// Update a thing shadow with `payload`.
    pub async fn update_shadow(
        &self,
        thing: &str,
        shadow: Option<String>,
        payload: Vec<u8>,
    ) -> Result<()> {
        let thing = thing.to_string();
        self.call(move |reply| Command::UpdateShadow {
            thing,
            shadow,
            payload,
            reply,
        })
        .await
    }

    /// Delete a thing shadow.
    pub async fn delete_shadow(&self, thing: &str, shadow: Option<String>) -> Result<()> {
        let thing = thing.to_string();
        self.call(move |reply| Command::DeleteShadow {
            thing,
            shadow,
            reply,
        })
        .await
    }
}

/// Map our [`Qos`] onto the SDK's IoT-Core QoS domain.
fn sdk_qos(qos: Qos) -> Result<gg_sdk::Qos> {
    match qos {
        Qos::AtMostOnce => Ok(gg_sdk::Qos::AtMostOnce),
        Qos::AtLeastOnce => Ok(gg_sdk::Qos::AtLeastOnce),
        Qos::ExactlyOnce => Err(EdgeCommonsError::Ipc(
            "QoS 2 is supported only on the local MQTT broker; Greengrass IoT Core supports QoS 0/1"
                .to_string(),
        )),
    }
}

/// Map an SDK error into our error type.
fn ipc_err(e: gg_sdk::Error) -> EdgeCommonsError {
    EdgeCommonsError::Ipc(format!("greengrass IPC error: {e}"))
}

/// Convert an SDK [`gg_sdk::Object`] tree into a `serde_json::Value`.
fn object_to_value(obj: gg_sdk::Object) -> Value {
    use gg_sdk::UnpackedObject as U;
    match obj.unpack() {
        U::Null => Value::Null,
        U::Bool(b) => Value::Bool(b),
        U::I64(i) => Value::from(i),
        U::F64(f) => serde_json::Number::from_f64(f).map_or(Value::Null, Value::Number),
        U::Buf(s) => Value::String(s.to_string()),
        U::List(list) => Value::Array(list.iter().map(|o| object_to_value(*o)).collect()),
        U::Map(map) => map_to_value(map),
    }
}

/// Convert an SDK [`gg_sdk::Map`] into a JSON object value.
fn map_to_value(map: gg_sdk::Map) -> Value {
    let mut obj = serde_json::Map::with_capacity(map.len());
    for kv in map.iter() {
        obj.insert(kv.key().to_string(), object_to_value(*kv.val()));
    }
    Value::Object(obj)
}

/// The worker loop: owns the `Sdk` and all live subscriptions; processes commands
/// serially. Subscription callbacks fire from SDK-internal threads and forward into
/// the bounded delivery channels (or, for config watches, post a `ConfigChanged`).
fn worker(rx: StdReceiver<Command>, internal_tx: StdSender<Command>) {
    let sdk = gg_sdk::Sdk::init();
    let mut connected = false;
    let mut next_id: u64 = 1;
    let mut subs: HashMap<u64, SubEntry> = HashMap::new();

    while let Ok(cmd) = rx.recv() {
        match cmd {
            Command::Connect(reply) => {
                let result = if connected {
                    Ok(())
                } else {
                    match sdk.connect() {
                        Ok(()) => {
                            connected = true;
                            Ok(())
                        }
                        Err(e) => Err(ipc_err(e)),
                    }
                };
                let _ = reply.send(result);
            }

            Command::Publish {
                topic,
                payload,
                dest,
                qos,
                reply,
            } => {
                let result = match dest {
                    Destination::Local => sdk
                        .publish_to_topic_binary(&topic, &payload)
                        .map_err(ipc_err),
                    Destination::Northbound => match sdk_qos(qos) {
                        Ok(mapped) => sdk
                            .publish_to_iot_core(&topic, &payload, mapped)
                            .map_err(ipc_err),
                        Err(e) => Err(e),
                    },
                };
                let _ = reply.send(result);
            }

            Command::Subscribe {
                filter,
                dest,
                qos,
                out,
                reply,
            } => {
                let id = next_id;
                next_id += 1;
                let result = match dest {
                    Destination::Local => start_local_sub(&sdk, &filter, out),
                    Destination::Northbound => start_iot_sub(&sdk, &filter, qos, out),
                };
                match result {
                    Ok(sub) => {
                        subs.insert(
                            id,
                            SubEntry {
                                _sub: sub,
                                config_watch: None,
                            },
                        );
                        let _ = reply.send(Ok(id));
                    }
                    Err(e) => {
                        let _ = reply.send(Err(e));
                    }
                }
            }

            Command::Unsubscribe { id } => {
                subs.remove(&id); // Drop closes the broker subscription.
            }

            Command::GetConfig {
                key_path,
                component,
                reply,
            } => {
                let _ = reply.send(do_get_config(&sdk, &key_path, component.as_deref()));
            }

            Command::WatchConfig {
                component,
                key_path,
                out,
                reply,
            } => {
                let id = next_id;
                next_id += 1;
                let cb_tx = internal_tx.clone();
                // Leak the concrete closure to obtain the `'static` borrow the SDK
                // requires (one small, bounded leak per subscription).
                let cb: &'static _ = Box::leak(Box::new(move |_comp: &str, _changed: &[&str]| {
                    let _ = cb_tx.send(Command::ConfigChanged { id });
                }));
                // Scope `kp` so its borrow of `key_path` ends before we move `key_path`.
                let sub_result = {
                    let kp: Vec<&str> = key_path.iter().map(String::as_str).collect();
                    sdk.subscribe_to_configuration_update(component.as_deref(), &kp, cb)
                };
                match sub_result {
                    Ok(sub) => {
                        subs.insert(
                            id,
                            SubEntry {
                                _sub: Box::new(sub),
                                config_watch: Some(ConfigWatch {
                                    component,
                                    key_path,
                                    out,
                                }),
                            },
                        );
                        let _ = reply.send(Ok(id));
                    }
                    Err(e) => {
                        let _ = reply.send(Err(ipc_err(e)));
                    }
                }
            }

            Command::ConfigChanged { id } => {
                if let Some(entry) = subs.get(&id) {
                    if let Some(cw) = &entry.config_watch {
                        if let Ok(value) =
                            do_get_config(&sdk, &cw.key_path, cw.component.as_deref())
                        {
                            let _ = cw.out.send(value);
                        }
                    }
                }
            }

            Command::GetShadow {
                thing,
                shadow,
                reply,
            } => {
                let mut buf = vec![MaybeUninit::<u8>::uninit(); IPC_RESULT_BUF];
                let result = sdk
                    .get_thing_shadow(&thing, shadow.as_deref(), &mut buf)
                    .map(<[u8]>::to_vec)
                    .map_err(ipc_err);
                let _ = reply.send(result);
            }

            Command::UpdateShadow {
                thing,
                shadow,
                payload,
                reply,
            } => {
                let result = sdk
                    .update_thing_shadow(&thing, shadow.as_deref(), &payload, None)
                    .map(|_| ())
                    .map_err(ipc_err);
                let _ = reply.send(result);
            }

            Command::DeleteShadow {
                thing,
                shadow,
                reply,
            } => {
                let result = sdk
                    .delete_thing_shadow(&thing, shadow.as_deref())
                    .map_err(ipc_err);
                let _ = reply.send(result);
            }
        }
    }
}

/// Run an SDK-callback body with panic containment.
///
/// SDK subscription callbacks fire on SDK-internal (C-FFI) threads. A panic
/// unwinding across that boundary is undefined behavior and, in practice, can wedge
/// the single Greengrass IPC event loop — the Rust analog of the Java
/// `SubscriptionHandler` worker dying on an uncaught callback exception. Containing
/// the panic here guarantees one bad message can never take down the subscription or
/// destabilize core IPC.
fn contain_callback(topic: &str, body: impl FnOnce()) {
    // `AssertUnwindSafe`: the only state the body touches across the catch boundary is
    // the delivery channel (internally consistent under a panic) and freshly-owned
    // locals; a panic leaves no broken shared invariant. SDK payload types do not
    // implement `UnwindSafe`, so assert it here rather than constrain the caller.
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)).is_err() {
        tracing::error!(
            topic = %topic,
            "IPC subscription callback panicked; suppressed so it cannot wedge the IPC event loop"
        );
    }
}

/// Start a local pub/sub subscription, returning the type-erased SDK subscription.
///
/// The callback is `Box::leak`ed to satisfy the SDK's `'static` borrow; the returned
/// `Subscription` keeps the broker subscription open until it is dropped. The
/// delivery body is panic-contained (it runs on an SDK thread) and late/duplicate
/// replies are dropped, not panicked on.
fn start_local_sub(sdk: &gg_sdk::Sdk, filter: &str, out: Delivery) -> Result<Box<dyn Any>> {
    use gg_sdk::SubscribeToTopicPayload as P;
    let cb: &'static _ = Box::leak(Box::new(move |topic: &str, payload: P| {
        let topic_owned = topic.to_string();
        contain_callback(topic, || {
            let bytes = match payload {
                P::Json(map) => serde_json::to_vec(&map_to_value(map)).unwrap_or_default(),
                P::Binary(b) => b.to_vec(),
            };
            // Best-effort, non-blocking delivery; a stray/late reply (closed or full
            // channel) is dropped and logged, never a panic.
            crate::messaging::request_reply::try_deliver_reply(&out, topic_owned, bytes);
        });
    }));
    let sub = sdk.subscribe_to_topic(filter, cb).map_err(ipc_err)?;
    Ok(Box::new(sub))
}

/// Start an IoT Core subscription, returning the type-erased SDK subscription.
///
/// As with [`start_local_sub`], the delivery body runs on an SDK thread and is
/// panic-contained, and late/duplicate replies are dropped rather than panicked on.
fn start_iot_sub(sdk: &gg_sdk::Sdk, filter: &str, qos: Qos, out: Delivery) -> Result<Box<dyn Any>> {
    let cb: &'static _ = Box::leak(Box::new(move |topic: &str, payload: &[u8]| {
        let topic_owned = topic.to_string();
        contain_callback(topic, || {
            crate::messaging::request_reply::try_deliver_reply(&out, topic_owned, payload.to_vec());
        });
    }));
    let sub = sdk
        .subscribe_to_iot_core(filter, sdk_qos(qos)?, cb)
        .map_err(ipc_err)?;
    Ok(Box::new(sub))
}

/// Run a `GetConfiguration` call and convert the result into a JSON document.
fn do_get_config(sdk: &gg_sdk::Sdk, key_path: &[String], component: Option<&str>) -> Result<Value> {
    let mut buf = vec![MaybeUninit::<u8>::uninit(); IPC_RESULT_BUF];
    let kp: Vec<&str> = key_path.iter().map(String::as_str).collect();
    let obj = sdk.get_config(&kp, component, &mut buf).map_err(ipc_err)?;
    Ok(object_to_value(obj))
}
