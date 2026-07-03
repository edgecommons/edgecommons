//! # Commands — the component command inbox + built-in verbs
//!
//! **One-liner purpose**: The library-owned component **command inbox** — the minimal
//! `commands()` facade (DESIGN-uns §7.3 / §9.5, edge-console slice S2), mirroring the Java
//! canonical `com.mbreissi.ggcommons.commands.CommandInbox`.
//!
//! ## Overview
//! Every ggcommons component subscribes, on its PRIMARY (local/IPC) connection, its own
//! `main`-instance command-inbox wildcard
//!
//! ```text
//! ecv1/{device}/{component}/main/cmd/#
//! ```
//!
//! and dispatches incoming `cmd` envelopes to handlers by **verb** — the topic's channel
//! (everything after `cmd/`, `/`-namespaced verbs included), which the envelope's
//! `header.name` must equal. A request carrying `header.reply_to` gets a structured reply on
//! that topic with the request's `correlation_id` (the `uns-bridge` rewrites `reply_to` across
//! brokers, so console→component request/reply works transparently over the site bus); a `cmd`
//! without `reply_to` is fire-and-forget (the handler runs, no reply). Obtain the facade via
//! [`crate::GgCommons::commands`] and register custom verbs with [`CommandInbox::register`].
//!
//! ## Normative behavior (mirrored by the Java/Python/TS inboxes; pinned by
//! `uns-test-vectors/commands.json`)
//! - **Reply body shape** — success `{"ok": true, "result": <verb-specific object>}`; error
//!   `{"ok": false, "error": {"code": <CODE>, "message": <text>}}`. The reply envelope's
//!   `header.name` is the verb, `header.version` is [`CMD_MESSAGE_VERSION`], and it carries the
//!   **responder's** `identity` (and `tags`, when configured — metadata, not normative).
//! - **Built-in verbs** (registered by the library, cannot be shadowed or unregistered):
//!   [`PING`] → `{"status": "RUNNING", "uptimeSecs": n}` (liveness/echo, the state keepalive's
//!   RUNNING body shape); [`RELOAD_CONFIG`] → re-fetch/re-apply the configuration from the
//!   active config source (`{"reloaded": true}` or [`ERR_RELOAD_FAILED`]); [`GET_CONFIGURATION`]
//!   → return the current **redacted effective config** as `{"config": <redacted config>}` — the
//!   same redacted snapshot the `cfg` push class publishes, as a reply (**Flow B**: the console
//!   pulls a component's own config; unrelated to the Flow-A
//!   `ecv1/{device}/config/main/cmd/get-configuration` rendezvous where a component fetches its
//!   config FROM a config server).
//! - **Unknown verb** — a well-formed request whose verb has no handler gets an
//!   [`ERR_UNKNOWN_VERB`] error reply (fire-and-forget unknowns are ignored at DEBUG).
//! - **Malformed** — a `header.name` that does not equal the topic's verb (which also covers a
//!   raw/non-envelope payload — its header defaults to an empty name) is ignored at DEBUG,
//!   **never replied to and never a crash** (the G-S1 precedent; replying would race foreign
//!   conventions that use a different header name on a `cmd` topic). Unlike the Java canonical,
//!   Rust's dispatcher always hands the handler a parsed [`Message`] (there is no "null message"
//!   case), so no separate null check is needed.
//! - **Delegated verbs** — [`SET_CONFIG_VERB`] is owned by the `CONFIG_COMPONENT` config
//!   source's own subscription on the same inbox path; the inbox always ignores it (DEBUG) so
//!   the two subscribers never double-handle.
//! - **Handler errors** — a [`CommandError`] keeps its code; fire-and-forget failures are logged
//!   only. Rust handlers must return a typed `Result<_, CommandError>` (no generic
//!   exception-catch-all as in Java) — use [`CommandError::handler_error`] for the generic
//!   [`ERR_HANDLER_ERROR`] code when a handler doesn't need a specific one.
//! - **No config surface** — always on; core plumbing, not a feature toggle.
//!
//! Lifecycle: constructed and [`CommandInbox::start`] by the `GgCommonsBuilder::build` runtime
//! right after the §9.4 [`crate::uns::RepublishListener`], whose wiring this module mirrors:
//! [`Weak`](std::sync::Weak) references in the subscribe callback so `Drop` still runs, and RAII
//! teardown (unsubscribe before the transport drops).
//!
//! ## Related Modules
//! - [`crate::uns`] — the topic builder/validator and the `_bcast` republish listener this
//!   module's wiring mirrors.
//! - [`crate::messaging`] — subscribe/reply.
//! - [`crate::config::effective`] — the redaction shared with the `cfg` push.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwap;
use async_trait::async_trait;
use serde_json::{json, Value};

use crate::config::model::Config;
use crate::error::{GgError, Result};
use crate::messaging::message::{Message, MessageBuilder};
use crate::messaging::{message_handler, MessagingService};
use crate::uns::{Uns, UnsClass, UnsScope};

/// The liveness/echo built-in verb.
pub const PING: &str = "ping";
/// The re-fetch/re-apply-configuration built-in verb.
pub const RELOAD_CONFIG: &str = "reload-config";
/// The return-my-redacted-effective-config built-in verb (Flow B).
pub const GET_CONFIGURATION: &str = "get-configuration";
/// The command request/reply envelope version.
pub const CMD_MESSAGE_VERSION: &str = "1.0";

/// Error code: the request's verb has no registered handler on this component.
pub const ERR_UNKNOWN_VERB: &str = "UNKNOWN_VERB";
/// Error code: the handler failed with a generic (uncoded) error.
pub const ERR_HANDLER_ERROR: &str = "HANDLER_ERROR";
/// Error code: [`RELOAD_CONFIG`] could not re-fetch or the document was rejected.
pub const ERR_RELOAD_FAILED: &str = "RELOAD_FAILED";
/// Error code: [`GET_CONFIGURATION`] found no effective configuration to return.
pub const ERR_NO_CONFIG: &str = "NO_CONFIG";

/// The `set-config` push verb — delegated: the `CONFIG_COMPONENT` config source maintains its
/// own subscription for it on the same inbox path, so the inbox must never dispatch or
/// error-reply it.
pub const SET_CONFIG_VERB: &str = "set-config";

/// The built-in verbs (registered at construction; shadowing/unregistering is rejected).
pub const BUILT_IN_VERBS: [&str; 3] = [PING, RELOAD_CONFIG, GET_CONFIGURATION];
/// Verbs owned by other library subscriptions on the same inbox path — always ignored.
pub const DELEGATED_VERBS: [&str; 1] = [SET_CONFIG_VERB];

/// The bounded client-side delivery queue for the inbox's single wildcard subscription.
const SUBSCRIBE_MAX_MESSAGES: usize = 32;
/// How many queued command deliveries the inbox processes concurrently (unlike the two-verb
/// `_bcast` republish listener, a busy console may address several distinct verbs at once).
const SUBSCRIBE_MAX_CONCURRENCY: usize = 4;

/// A coded command failure (DESIGN-uns §9.5): returned by a [`CommandHandler`] to produce a
/// structured error reply `{"ok": false, "error": {"code": <code>, "message": <message>}}` with
/// a caller-chosen machine-readable code. This is the Rust analog of the Java canonical's
/// `CommandException` — Rust handlers return it directly (via `Result`) rather than throwing.
#[derive(Debug, Clone)]
pub struct CommandError {
    /// The machine-readable error code carried in the error reply's `error.code`
    /// (SCREAMING_SNAKE_CASE by convention — see the pinned base codes on this module).
    pub code: String,
    /// The human-readable message carried in the error reply's `error.message`.
    pub message: String,
}

impl CommandError {
    /// Creates a coded command error.
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self { code: code.into(), message: message.into() }
    }

    /// A generic, uncoded failure — [`ERR_HANDLER_ERROR`] with `message`'s `Display` text. The
    /// idiomatic Rust equivalent of the Java canonical letting an *uncoded* exception fall
    /// through to the generic code: since [`CommandHandler::handle`] requires a typed
    /// [`CommandError`] (there is no catch-all), a handler that doesn't care to pick a specific
    /// code maps any error into one with `?`/`.map_err(CommandError::handler_error)`.
    pub fn handler_error(message: impl std::fmt::Display) -> Self {
        Self::new(ERR_HANDLER_ERROR, message.to_string())
    }
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

impl std::error::Error for CommandError {}

/// A command-verb handler (DESIGN-uns §9.5): invoked by the [`CommandInbox`] for every
/// well-formed `cmd` envelope whose verb matches the registration.
///
/// The `Ok` value is the verb-specific **result object**, wrapped by the inbox into the success
/// reply body `{"ok": true, "result": <value>}` and published to the request's `header.reply_to`
/// (with the request's `correlation_id`). `Ok(None)` yields an empty result (`{"ok": true,
/// "result": {}}` — a plain acknowledgement). When the request carries no `reply_to`
/// (fire-and-forget) the handler still runs but the result is discarded.
///
/// Failures: return a [`CommandError`] for a coded error reply. Handlers run on the messaging
/// dispatcher (bounded concurrency, see [`SUBSCRIBE_MAX_CONCURRENCY`]) — keep them fast, or hand
/// off internally.
#[async_trait]
pub trait CommandHandler: Send + Sync + 'static {
    /// Handles one command request. `request` is the full request envelope (body = the verb's
    /// arguments object; the requester's `identity`/`tags`, when present, are informational).
    async fn handle(&self, request: Message) -> std::result::Result<Option<Value>, CommandError>;
}

/// Adapts an async closure into a [`CommandHandler`].
struct FnCommandHandler<F>(F);

#[async_trait]
impl<F, Fut> CommandHandler for FnCommandHandler<F>
where
    F: Fn(Message) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = std::result::Result<Option<Value>, CommandError>> + Send + 'static,
{
    async fn handle(&self, request: Message) -> std::result::Result<Option<Value>, CommandError> {
        (self.0)(request).await
    }
}

/// Wrap an async closure as a [`CommandHandler`] for [`CommandInbox::register`].
///
/// # Examples
/// ```
/// use ggcommons::commands::command_handler;
/// use serde_json::json;
/// let _h = command_handler(|_request| async move { Ok(Some(json!({ "restarted": true }))) });
/// ```
pub fn command_handler<F, Fut>(f: F) -> Arc<dyn CommandHandler>
where
    F: Fn(Message) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = std::result::Result<Option<Value>, CommandError>> + Send + 'static,
{
    Arc::new(FnCommandHandler(f))
}

/// One out-of-band re-fetch-and-apply action (the [`RELOAD_CONFIG`] verb's action): re-invokes
/// the active config source's `load()`, validates, and — only on success — atomically applies the
/// new snapshot and notifies listeners (production: [`crate::apply_reloaded_config`] over the
/// source captured at build time). Returns `true` on success. An infallible, best-effort async
/// callback — failures are logged internally, mirroring [`crate::uns::RepublishAction`].
pub(crate) type ReloadAction = Arc<dyn Fn() -> Pin<Box<dyn Future<Output = bool> + Send>> + Send + Sync>;

/// Lifecycle flags + the resolved inbox topic, behind one lock (no `.await` ever happens while
/// holding it) — mirrors [`crate::uns::RepublishListener`]'s `Inner`.
#[derive(Default)]
struct Inner {
    started: bool,
    closed: bool,
    /// The subscribed inbox filter (`…/cmd/#`); `None` until [`CommandInbox::start`] builds it.
    inbox_filter: Option<String>,
    /// The filter minus the trailing `#` — the verb-extraction prefix (`…/cmd/`). Set BEFORE
    /// subscribing (mirrors the Java canonical) so a delivery racing the subscribe call still
    /// resolves the verb correctly.
    inbox_prefix: Option<String>,
}

/// The library-owned component **command inbox** — see the [module docs](self).
pub struct CommandInbox {
    messaging: Arc<dyn MessagingService>,
    config: Arc<ArcSwap<Config>>,
    /// verb → handler; built-ins seeded at construction, custom verbs via [`Self::register`].
    handlers: Mutex<HashMap<String, Arc<dyn CommandHandler>>>,
    inner: Mutex<Inner>,
}

impl CommandInbox {
    /// Creates the inbox and registers the three built-in verbs. The verb *actions* are injected
    /// seams so the built-ins unit-test deterministically; `GgCommonsBuilder::build` wires the
    /// real ones.
    ///
    /// - `uptime_secs` — the [`PING`] uptime source (production: the heartbeat's monotonic
    ///   uptime, [`crate::heartbeat::Heartbeat::uptime_secs`]).
    /// - `reload_action` — the [`RELOAD_CONFIG`] action (production: re-fetch + re-apply from the
    ///   active config source, sharing the same apply path a watched hot-reload uses).
    /// - `redacted_config` — the [`GET_CONFIGURATION`] source: the current redacted effective
    ///   config, or `None` when unavailable (production: [`crate::config::effective::redact`]
    ///   over the live config snapshot — always `Some` once `build()` has succeeded; kept
    ///   optional for parity with the Java canonical's mock/test bring-up case and so
    ///   [`ERR_NO_CONFIG`] is directly testable).
    pub(crate) fn new(
        messaging: Arc<dyn MessagingService>,
        config: Arc<ArcSwap<Config>>,
        uptime_secs: Arc<dyn Fn() -> u64 + Send + Sync>,
        reload_action: ReloadAction,
        redacted_config: Arc<dyn Fn() -> Option<Value> + Send + Sync>,
    ) -> Arc<CommandInbox> {
        let mut handlers: HashMap<String, Arc<dyn CommandHandler>> = HashMap::new();

        // ping -> the state keepalive's RUNNING body shape: proves the component is not just
        // alive (the keepalive does that) but RESPONSIVE to addressed commands.
        handlers.insert(
            PING.to_string(),
            command_handler(move |_request| {
                let uptime_secs = uptime_secs.clone();
                async move { Ok(Some(json!({ "status": "RUNNING", "uptimeSecs": (uptime_secs)() }))) }
            }),
        );

        // reload-config -> re-fetch from the active config source and re-apply (listeners fire,
        // so a successful reload also re-announces the cfg push as a side effect).
        handlers.insert(
            RELOAD_CONFIG.to_string(),
            command_handler(move |_request| {
                let reload_action = reload_action.clone();
                async move {
                    if (reload_action)().await {
                        Ok(Some(json!({ "reloaded": true })))
                    } else {
                        Err(CommandError::new(
                            ERR_RELOAD_FAILED,
                            "the configuration could not be re-fetched from the active config \
                             source or the document was rejected - see the component log",
                        ))
                    }
                }
            }),
        );

        // get-configuration (Flow B) -> the cfg class's body shape, as a reply.
        handlers.insert(
            GET_CONFIGURATION.to_string(),
            command_handler(move |_request| {
                let redacted_config = redacted_config.clone();
                async move {
                    match (redacted_config)() {
                        Some(config) => Ok(Some(json!({ "config": config }))),
                        None => Err(CommandError::new(
                            ERR_NO_CONFIG,
                            "no effective configuration is available",
                        )),
                    }
                }
            }),
        );

        Arc::new(CommandInbox {
            messaging,
            config,
            handlers: Mutex::new(handlers),
            inner: Mutex::new(Inner::default()),
        })
    }

    /// Registers a custom verb handler — the minimal `commands()` registration seam. The verb is
    /// one or more `/`-separated channel tokens (`"restart-pipeline"`, `"sb/status"`), each
    /// validated against the §2.2 token rule. Registration is allowed before or after
    /// [`Self::start`] (the inbox is a single wildcard subscription — no per-verb subscribe).
    ///
    /// **Precedence:** no shadowing, ever — registering a built-in, a delegated
    /// ([`DELEGATED_VERBS`]) or an already-registered verb errors. Replace a custom handler by
    /// [`Self::unregister`] first.
    ///
    /// # Errors
    /// [`GgError::UnsValidation`] when a verb token violates the §2.2 token rule;
    /// [`GgError::Command`] when the verb is built-in/delegated/already registered.
    pub fn register(&self, verb: &str, handler: Arc<dyn CommandHandler>) -> Result<()> {
        for token in verb.split('/') {
            crate::uns::check_token(token, "verb token")?;
        }
        if BUILT_IN_VERBS.contains(&verb) {
            return Err(GgError::Command(format!(
                "verb '{verb}' is a built-in verb and cannot be shadowed"
            )));
        }
        if DELEGATED_VERBS.contains(&verb) {
            return Err(GgError::Command(format!(
                "verb '{verb}' is owned by another library subsystem and cannot be registered"
            )));
        }
        let mut handlers = self.handlers.lock().unwrap();
        if handlers.contains_key(verb) {
            return Err(GgError::Command(format!(
                "verb '{verb}' is already registered - unregister it first to replace the handler"
            )));
        }
        handlers.insert(verb.to_string(), handler);
        tracing::debug!(verb, "command verb registered");
        Ok(())
    }

    /// Removes a previously registered custom verb handler. Unknown verbs are a no-op; built-in
    /// verbs cannot be unregistered.
    ///
    /// # Errors
    /// [`GgError::Command`] when `verb` is a built-in.
    pub fn unregister(&self, verb: &str) -> Result<()> {
        if BUILT_IN_VERBS.contains(&verb) {
            return Err(GgError::Command(format!(
                "verb '{verb}' is a built-in verb and cannot be unregistered"
            )));
        }
        if self.handlers.lock().unwrap().remove(verb).is_some() {
            tracing::debug!(verb, "command verb unregistered");
        }
        Ok(())
    }

    /// The currently registered verbs (built-ins + custom) — a snapshot copy.
    pub fn verbs(&self) -> std::collections::HashSet<String> {
        self.handlers.lock().unwrap().keys().cloned().collect()
    }

    /// Builds the own-inbox wildcard (`ecv1/{device}/{component}/main/cmd/#`, through the topic
    /// builder under this component's identity + root mode) and subscribes it on the PRIMARY
    /// connection. Best-effort and idempotent: on any topic-build or subscribe failure the inbox
    /// logs a WARN and disables itself (returns without setting `started`) — the component must
    /// come up regardless.
    ///
    /// The subscribe handler holds only a [`std::sync::Weak`] reference back to this inbox
    /// (never a strong one) — otherwise the inbox could never be dropped while its own
    /// subscription is live, and [`Drop::drop`] (which unsubscribes it) would never run.
    pub(crate) async fn start(self: Arc<Self>) {
        {
            let inner = self.inner.lock().unwrap();
            if inner.started || inner.closed {
                return;
            }
        }

        let snapshot = self.config.load_full();
        let identity = snapshot.identity();
        let uns = Uns::new(identity.clone(), snapshot.topic_include_root());
        // Pin every scope token to this component's own identity: the site value is consulted
        // only under an effective root mode (D-U25 makes it a no-op otherwise).
        let site =
            if identity.hier().len() >= 2 { Some(identity.hier()[0].value.clone()) } else { None };
        let scope = UnsScope {
            site,
            device: Some(identity.device().to_string()),
            component: Some(identity.component().to_string()),
            instance: Some(identity.instance().to_string()),
        };
        let filter = match uns.filter(UnsClass::Cmd, &scope) {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "failed to build the command inbox filter - the command inbox is disabled"
                );
                return;
            }
        };

        // ".../cmd/#" -> ".../cmd/" - the verb is the topic's remainder after this prefix.
        // Assigned BEFORE subscribing so a delivery racing the subscribe call sees it.
        let prefix = filter[..filter.len() - 1].to_string();
        {
            let mut inner = self.inner.lock().unwrap();
            inner.inbox_filter = Some(filter.clone());
            inner.inbox_prefix = Some(prefix);
        }

        let weak = Arc::downgrade(&self);
        let handler = message_handler(move |topic, message| {
            let weak = weak.clone();
            async move {
                if let Some(inbox) = weak.upgrade() {
                    CommandInbox::handle(inbox, topic, message).await;
                }
            }
        });
        if let Err(e) = self
            .messaging
            .subscribe(&filter, handler, SUBSCRIBE_MAX_MESSAGES, SUBSCRIBE_MAX_CONCURRENCY)
            .await
        {
            tracing::warn!(
                error = %e,
                filter,
                "failed to subscribe the command inbox - continuing without it"
            );
            return;
        }

        let mut inner = self.inner.lock().unwrap();
        inner.started = true;
        drop(inner);
        tracing::info!(filter, verbs = ?self.verbs(), "command inbox subscribed");
    }

    /// One received `cmd` envelope: extract the verb from the topic, validate the envelope
    /// (`header.name` must equal the verb), dispatch, reply. Never panics — a malformed or
    /// foreign payload is ignored at DEBUG.
    async fn handle(inbox: Arc<Self>, topic: String, message: Message) {
        let (closed, prefix) = {
            let inner = inbox.inner.lock().unwrap();
            (inner.closed, inner.inbox_prefix.clone())
        };
        if closed {
            return;
        }
        let Some(prefix) = prefix else {
            // Should not happen (the prefix is set before the subscribe callback can fire), but
            // guards against any future re-ordering.
            tracing::debug!(topic = %topic, "command inbox delivery before the prefix was set; ignoring");
            return;
        };
        if !topic.starts_with(&prefix) {
            // ".../cmd/#" also matches the bare ".../cmd" parent level - nothing to dispatch.
            tracing::debug!(topic = %topic, "ignoring cmd delivery outside the inbox prefix");
            return;
        }
        let verb = &topic[prefix.len()..];
        if verb.is_empty() {
            return;
        }
        if DELEGATED_VERBS.contains(&verb) {
            tracing::debug!(
                verb,
                "ignoring delegated verb (owned by another library subscription)"
            );
            return;
        }
        if message.is_raw() || message.header.name != verb {
            // Malformed/foreign: never replied to (a reply would race foreign conventions using
            // a different header name on a cmd topic), never a crash.
            tracing::debug!(
                topic = %topic,
                "ignoring malformed/foreign cmd payload (header.name must equal the topic verb)"
            );
            return;
        }
        Self::dispatch(inbox, verb.to_string(), message).await;
    }

    /// Dispatches a well-formed request to its handler and replies (when `reply_to` is set).
    async fn dispatch(inbox: Arc<Self>, verb: String, request: Message) {
        let wants_reply = request.header.reply_to.as_deref().is_some_and(|s| !s.is_empty());
        let handler = { inbox.handlers.lock().unwrap().get(&verb).cloned() };
        let Some(handler) = handler else {
            if wants_reply {
                tracing::debug!(verb, code = ERR_UNKNOWN_VERB, "unknown verb; sending error reply");
                inbox
                    .send_reply(
                        &request,
                        &verb,
                        error_body(
                            ERR_UNKNOWN_VERB,
                            format!("verb '{verb}' is not registered on this component"),
                        ),
                    )
                    .await;
            } else {
                tracing::debug!(verb, "ignoring unknown fire-and-forget verb");
            }
            return;
        };

        // One clone: the handler takes ownership of its request, while `request` is kept for
        // the reply (which needs the ORIGINAL request's reply_to/correlation_id).
        match handler.handle(request.clone()).await {
            Ok(result) => {
                if wants_reply {
                    let body = json!({ "ok": true, "result": result.unwrap_or_else(|| json!({})) });
                    inbox.send_reply(&request, &verb, body).await;
                }
            }
            Err(e) => {
                if wants_reply {
                    inbox.send_reply(&request, &verb, error_body(&e.code, e.message)).await;
                } else {
                    tracing::warn!(verb, code = %e.code, message = %e.message, "fire-and-forget verb failed");
                }
            }
        }
    }

    /// Publishes a reply to the request's `reply_to` through the existing reply mechanism (the
    /// provider stamps the request's `correlation_id` onto the reply). The reply is
    /// config-stamped, so it carries the responder's `identity` (+ `tags`). Best-effort: a
    /// failing reply (e.g. a hostile reserved-class `reply_to` rejected by the guard) is logged
    /// and swallowed.
    async fn send_reply(&self, request: &Message, verb: &str, body: Value) {
        let snapshot = self.config.load_full();
        let reply = MessageBuilder::new(verb, CMD_MESSAGE_VERSION)
            .payload(body)
            .from_config(&snapshot)
            .build();
        if let Err(e) = self.messaging.reply(request, reply).await {
            tracing::warn!(error = %e, verb, "command reply failed");
        }
    }

    /// Marks the inbox closed (idempotent) and returns the filter to unsubscribe (`None` if
    /// already closed or never started). Shared by [`Self::close`] and [`Drop::drop`].
    fn mark_closed(&self) -> Option<String> {
        let mut inner = self.inner.lock().unwrap();
        if inner.closed {
            return None;
        }
        inner.closed = true;
        if inner.started { inner.inbox_filter.clone() } else { None }
    }

    /// Test-only deterministic teardown: the same unsubscribe-before-exit logic as
    /// [`Drop::drop`], but awaited synchronously (no fire-and-forget spawn), so tests can assert
    /// the post-close state without polling/sleeping. Idempotent. Production teardown is
    /// RAII-only (`Drop`, mirroring [`crate::uns::RepublishListener`]) — this is not part of the
    /// production wiring, hence `#[cfg(test)]`.
    #[cfg(test)]
    pub(crate) async fn close(&self) {
        if let Some(filter) = self.mark_closed() {
            if let Err(e) = self.messaging.unsubscribe(&filter).await {
                tracing::debug!(error = %e, filter, "command-inbox unsubscribe failed");
            }
        }
    }
}

impl Drop for CommandInbox {
    /// RAII teardown (mirrors [`crate::uns::RepublishListener`] / [`crate::heartbeat::Heartbeat`]):
    /// unsubscribes the inbox wildcard — while messaging is still up (the unsubscribe-before-exit
    /// rule) — on a spawned fire-and-forget task, since `Drop` cannot `.await`. A no-op when
    /// never started, already closed, or no `tokio` runtime is available to spawn on.
    fn drop(&mut self) {
        let Some(filter) = self.mark_closed() else { return };
        let messaging = self.messaging.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                if let Err(e) = messaging.unsubscribe(&filter).await {
                    tracing::debug!(error = %e, filter, "command-inbox unsubscribe failed");
                }
            });
        }
    }
}

/// The error reply body `{"ok": false, "error": {"code", "message"}}`.
fn error_body(code: &str, message: impl Into<String>) -> Value {
    json!({ "ok": false, "error": { "code": code, "message": message.into() } })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::RecordingMessaging;
    use serde_json::json;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    const INBOX_FILTER: &str = "ecv1/test-thing/TestComponent/main/cmd/#";
    const REPLY_TO: &str = "ggcommons/reply-test-1";

    fn test_config() -> Arc<ArcSwap<Config>> {
        Arc::new(ArcSwap::from_pointee(
            Config::from_value("TestComponent", "test-thing", json!({})).unwrap(),
        ))
    }

    fn topic(verb: &str) -> String {
        format!("ecv1/test-thing/TestComponent/main/cmd/{verb}")
    }

    /// A well-formed request for a verb: `header.name` = verb, pinned `reply_to`.
    fn request(verb: &str) -> Message {
        MessageBuilder::new(verb, "1.0").payload(json!({})).reply_to(REPLY_TO).build()
    }

    /// A well-formed fire-and-forget command (no `reply_to`).
    fn notification(verb: &str) -> Message {
        MessageBuilder::new(verb, "1.0").payload(json!({})).build()
    }

    /// A deterministic fixture: injected uptime/reload/redacted-config seams over a
    /// [`RecordingMessaging`], mirroring the Java `CommandInboxTest` fixture.
    struct Fixture {
        messaging: Arc<RecordingMessaging>,
        uptime: Arc<AtomicU64>,
        reload_ok: Arc<AtomicBool>,
        redacted: Arc<Mutex<Option<Value>>>,
        inbox: Arc<CommandInbox>,
    }

    fn fixture() -> Fixture {
        let messaging = RecordingMessaging::new();
        let config = test_config();
        let uptime = Arc::new(AtomicU64::new(42));
        let reload_ok = Arc::new(AtomicBool::new(true));
        let redacted =
            Arc::new(Mutex::new(Some(json!({ "component": { "global": { "v": 1 } } }))));

        let uptime_secs: Arc<dyn Fn() -> u64 + Send + Sync> = {
            let uptime = uptime.clone();
            Arc::new(move || uptime.load(Ordering::SeqCst))
        };
        let reload_action: ReloadAction = {
            let reload_ok = reload_ok.clone();
            Arc::new(move || {
                let reload_ok = reload_ok.clone();
                Box::pin(async move { reload_ok.load(Ordering::SeqCst) })
            })
        };
        let redacted_config: Arc<dyn Fn() -> Option<Value> + Send + Sync> = {
            let redacted = redacted.clone();
            Arc::new(move || redacted.lock().unwrap().clone())
        };

        let inbox =
            CommandInbox::new(messaging.clone(), config, uptime_secs, reload_action, redacted_config);
        Fixture { messaging, uptime, reload_ok, redacted, inbox }
    }

    /// The single recorded reply (topic must be the request's `reply_to`).
    fn only_reply_body(messaging: &RecordingMessaging) -> Value {
        let replies = messaging.replies();
        assert_eq!(replies.len(), 1, "exactly one reply expected");
        let (topic, msg) = &replies[0];
        assert_eq!(topic, REPLY_TO, "the reply must go to the request's reply_to");
        msg.body.clone()
    }

    // ===================== subscription lifecycle =====================

    #[tokio::test]
    async fn start_subscribes_the_own_inbox_wildcard() {
        let f = fixture();
        f.inbox.clone().start().await;
        assert_eq!(
            f.messaging.subscribed_topics(),
            std::collections::HashSet::from([INBOX_FILTER.to_string()]),
            "start() must subscribe exactly the own-inbox cmd wildcard"
        );
    }

    #[tokio::test]
    async fn start_is_idempotent() {
        let f = fixture();
        f.inbox.clone().start().await;
        f.inbox.clone().start().await;
        assert_eq!(
            f.messaging.subscribed_topics(),
            std::collections::HashSet::from([INBOX_FILTER.to_string()])
        );
    }

    #[tokio::test]
    async fn close_unsubscribes_and_stops_dispatch() {
        let f = fixture();
        f.inbox.clone().start().await;
        f.inbox.close().await;
        assert!(
            f.messaging.subscribed_topics().is_empty(),
            "close() must unsubscribe the inbox (unsubscribe-before-exit)"
        );
        // A late (queued) delivery after close is ignored.
        f.messaging.simulate_message(&topic(PING), request(PING)).await;
        assert!(f.messaging.replies().is_empty());
    }

    #[tokio::test]
    async fn close_is_idempotent_and_start_after_close_is_a_noop() {
        let f = fixture();
        f.inbox.clone().start().await;
        f.inbox.close().await;
        f.inbox.close().await; // idempotent, must not panic
        f.inbox.clone().start().await; // closed -> must not resubscribe
        assert!(f.messaging.subscribed_topics().is_empty());
    }

    // ===================== built-in verbs =====================

    #[tokio::test]
    async fn ping_replies_status_and_uptime() {
        let f = fixture();
        f.uptime.store(1234, Ordering::SeqCst);
        f.inbox.clone().start().await;
        f.messaging.simulate_message(&topic(PING), request(PING)).await;
        let body = only_reply_body(&f.messaging);
        assert!(body["ok"].as_bool().unwrap());
        assert_eq!(body["result"]["status"], "RUNNING");
        assert_eq!(body["result"]["uptimeSecs"], 1234);
    }

    #[tokio::test]
    async fn reply_carries_the_request_correlation_id_verb_name_and_responder_identity() {
        let f = fixture();
        f.inbox.clone().start().await;
        let ping = request(PING);
        f.messaging.simulate_message(&topic(PING), ping.clone()).await;
        let replies = f.messaging.replies();
        let (_, reply) = &replies[0];
        assert_eq!(
            reply.header.correlation_id, ping.header.correlation_id,
            "the reply must carry the request's correlation_id"
        );
        assert_eq!(reply.header.name, PING, "the reply header.name is the verb");
        assert_eq!(reply.header.version, CMD_MESSAGE_VERSION);
        assert!(reply.identity.is_some(), "the reply is config-stamped with the responder's identity");
    }

    #[tokio::test]
    async fn reload_config_replies_ack_on_success() {
        let f = fixture();
        f.inbox.clone().start().await;
        f.messaging.simulate_message(&topic(RELOAD_CONFIG), request(RELOAD_CONFIG)).await;
        let body = only_reply_body(&f.messaging);
        assert!(body["ok"].as_bool().unwrap());
        assert!(body["result"]["reloaded"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn reload_config_replies_reload_failed_on_failure() {
        let f = fixture();
        f.reload_ok.store(false, Ordering::SeqCst);
        f.inbox.clone().start().await;
        f.messaging.simulate_message(&topic(RELOAD_CONFIG), request(RELOAD_CONFIG)).await;
        let body = only_reply_body(&f.messaging);
        assert!(!body["ok"].as_bool().unwrap());
        assert_eq!(body["error"]["code"], ERR_RELOAD_FAILED);
        assert!(!body["error"]["message"].as_str().unwrap().is_empty());
    }

    #[tokio::test]
    async fn get_configuration_replies_the_redacted_effective_config() {
        let f = fixture();
        f.inbox.clone().start().await;
        f.messaging.simulate_message(&topic(GET_CONFIGURATION), request(GET_CONFIGURATION)).await;
        let body = only_reply_body(&f.messaging);
        assert!(body["ok"].as_bool().unwrap());
        assert_eq!(
            body["result"]["config"],
            f.redacted.lock().unwrap().clone().unwrap(),
            "get-configuration must return the redacted effective config (Flow B)"
        );
    }

    #[tokio::test]
    async fn get_configuration_replies_no_config_when_unavailable() {
        let f = fixture();
        *f.redacted.lock().unwrap() = None;
        f.inbox.clone().start().await;
        f.messaging.simulate_message(&topic(GET_CONFIGURATION), request(GET_CONFIGURATION)).await;
        let body = only_reply_body(&f.messaging);
        assert!(!body["ok"].as_bool().unwrap());
        assert_eq!(body["error"]["code"], ERR_NO_CONFIG);
    }

    // ===================== custom verbs (the registration seam) =====================

    #[tokio::test]
    async fn custom_verb_registers_and_dispatches() {
        let f = fixture();
        f.inbox.clone().start().await; // registration after start needs no new subscription
        f.inbox
            .register(
                "restart-pipeline",
                command_handler(|_req| async move { Ok(Some(json!({ "restarted": true }))) }),
            )
            .unwrap();
        f.messaging.simulate_message(&topic("restart-pipeline"), request("restart-pipeline")).await;
        let body = only_reply_body(&f.messaging);
        assert!(body["ok"].as_bool().unwrap());
        assert!(body["result"]["restarted"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn namespaced_custom_verb_dispatches() {
        let f = fixture();
        f.inbox.register("sb/status", command_handler(|_req| async move { Ok(None) })).unwrap();
        f.inbox.clone().start().await;
        f.messaging.simulate_message(&topic("sb/status"), request("sb/status")).await;
        let body = only_reply_body(&f.messaging);
        assert!(body["ok"].as_bool().unwrap());
        assert_eq!(body["result"], json!({}), "a None handler result must reply an empty result object");
    }

    #[tokio::test]
    async fn handler_command_error_keeps_its_code() {
        let f = fixture();
        f.inbox
            .register(
                "guarded",
                command_handler(|_req| async move {
                    Err(CommandError::new("NOT_ALLOWED", "operator role required"))
                }),
            )
            .unwrap();
        f.inbox.clone().start().await;
        f.messaging.simulate_message(&topic("guarded"), request("guarded")).await;
        let body = only_reply_body(&f.messaging);
        assert!(!body["ok"].as_bool().unwrap());
        assert_eq!(body["error"]["code"], "NOT_ALLOWED");
        assert_eq!(body["error"]["message"], "operator role required");
    }

    #[tokio::test]
    async fn handler_error_maps_to_handler_error_via_the_convenience_constructor() {
        let f = fixture();
        f.inbox
            .register(
                "boomy",
                command_handler(|_req| async move { Err(CommandError::handler_error("boom")) }),
            )
            .unwrap();
        f.inbox.clone().start().await;
        f.messaging.simulate_message(&topic("boomy"), request("boomy")).await;
        let body = only_reply_body(&f.messaging);
        assert!(!body["ok"].as_bool().unwrap());
        assert_eq!(body["error"]["code"], ERR_HANDLER_ERROR);
    }

    #[tokio::test]
    async fn register_rejects_shadowing_and_invalid_verbs() {
        let f = fixture();
        assert!(matches!(
            f.inbox.register(PING, command_handler(|_r| async move { Ok(None) })),
            Err(GgError::Command(_))
        ));
        assert!(matches!(
            f.inbox.register(SET_CONFIG_VERB, command_handler(|_r| async move { Ok(None) })),
            Err(GgError::Command(_))
        ));
        f.inbox.register("mine", command_handler(|_r| async move { Ok(None) })).unwrap();
        assert!(matches!(
            f.inbox.register("mine", command_handler(|_r| async move { Ok(None) })),
            Err(GgError::Command(_))
        ));
        assert!(matches!(
            f.inbox.register("bad+verb", command_handler(|_r| async move { Ok(None) })),
            Err(GgError::UnsValidation { .. })
        ));
        assert!(matches!(
            f.inbox.register("sb//x", command_handler(|_r| async move { Ok(None) })),
            Err(GgError::UnsValidation { .. })
        ));
    }

    #[tokio::test]
    async fn unregister_removes_custom_verbs_but_never_built_ins() {
        let f = fixture();
        f.inbox.register("mine", command_handler(|_r| async move { Ok(None) })).unwrap();
        assert!(f.inbox.verbs().contains("mine"));
        f.inbox.unregister("mine").unwrap();
        assert!(!f.inbox.verbs().contains("mine"));
        f.inbox.unregister("mine").unwrap(); // unknown -> no-op
        assert!(matches!(f.inbox.unregister(RELOAD_CONFIG), Err(GgError::Command(_))));

        // The unregistered verb now gets the unknown-verb error.
        f.inbox.clone().start().await;
        f.messaging.simulate_message(&topic("mine"), request("mine")).await;
        assert_eq!(only_reply_body(&f.messaging)["error"]["code"], ERR_UNKNOWN_VERB);
    }

    #[tokio::test]
    async fn verbs_snapshot_contains_built_ins_and_customs() {
        let f = fixture();
        f.inbox.register("mine", command_handler(|_r| async move { Ok(None) })).unwrap();
        assert_eq!(
            f.inbox.verbs(),
            std::collections::HashSet::from([
                PING.to_string(),
                RELOAD_CONFIG.to_string(),
                GET_CONFIGURATION.to_string(),
                "mine".to_string(),
            ])
        );
    }

    // ===================== unknown / fire-and-forget / malformed =====================

    #[tokio::test]
    async fn unknown_verb_request_gets_an_unknown_verb_error_reply() {
        let f = fixture();
        f.inbox.clone().start().await;
        f.messaging.simulate_message(&topic("no-such-verb"), request("no-such-verb")).await;
        let body = only_reply_body(&f.messaging);
        assert!(!body["ok"].as_bool().unwrap());
        assert_eq!(body["error"]["code"], ERR_UNKNOWN_VERB);
    }

    #[tokio::test]
    async fn unknown_fire_and_forget_verb_is_ignored() {
        let f = fixture();
        f.inbox.clone().start().await;
        f.messaging.simulate_message(&topic("no-such-verb"), notification("no-such-verb")).await;
        assert!(
            f.messaging.replies().is_empty(),
            "an unknown fire-and-forget verb must not be replied to"
        );
    }

    #[tokio::test]
    async fn no_reply_to_runs_the_handler_without_replying() {
        let f = fixture();
        let ran = Arc::new(AtomicBool::new(false));
        let ran_handler = ran.clone();
        f.inbox
            .register(
                "do-it",
                command_handler(move |_req| {
                    let ran_handler = ran_handler.clone();
                    async move {
                        ran_handler.store(true, Ordering::SeqCst);
                        Ok(None)
                    }
                }),
            )
            .unwrap();
        f.inbox.clone().start().await;
        f.messaging.simulate_message(&topic("do-it"), notification("do-it")).await;
        assert!(ran.load(Ordering::SeqCst), "a fire-and-forget command must still run the handler");
        assert!(f.messaging.replies().is_empty(), "...but never reply");
    }

    #[tokio::test]
    async fn fire_and_forget_handler_failure_is_logged_only() {
        let f = fixture();
        f.inbox
            .register(
                "do-it",
                command_handler(|_req| async move { Err(CommandError::new("NOPE", "nope")) }),
            )
            .unwrap();
        f.inbox.clone().start().await;
        f.messaging.simulate_message(&topic("do-it"), notification("do-it")).await;
        assert!(f.messaging.replies().is_empty());
    }

    #[tokio::test]
    async fn malformed_payloads_are_ignored_without_reply_and_never_crash() {
        let f = fixture();
        f.inbox.clone().start().await;
        // header.name does not equal the topic verb (foreign convention on a cmd topic).
        f.messaging.simulate_message(&topic(PING), request("something-else")).await;
        // A raw (headerless) envelope - junk JSON on the inbox.
        f.messaging.simulate_message(&topic(PING), Message::raw(json!({ "junk": true }))).await;
        assert!(f.messaging.replies().is_empty(), "malformed/foreign payloads must never be replied to");
    }

    #[tokio::test]
    async fn delegated_set_config_is_ignored_even_as_a_request() {
        let f = fixture();
        f.inbox.clone().start().await;
        f.messaging.simulate_message(&topic(SET_CONFIG_VERB), request(SET_CONFIG_VERB)).await;
        assert!(
            f.messaging.replies().is_empty(),
            "set-config is owned by the CONFIG_COMPONENT subscription - never dispatched or replied to here"
        );
    }

    #[tokio::test]
    async fn bare_cmd_parent_level_delivery_is_ignored() {
        let f = fixture();
        f.inbox.clone().start().await;
        // MQTT "#" also matches the parent level (".../cmd") - nothing to dispatch there.
        f.messaging
            .simulate_message("ecv1/test-thing/TestComponent/main/cmd", request(PING))
            .await;
        assert!(f.messaging.replies().is_empty());
    }

    #[tokio::test]
    async fn a_failing_reply_publish_is_swallowed() {
        let f = fixture();
        f.inbox.clone().start().await;
        f.messaging.set_fail_reply(true);
        // Must not panic even though the reply publish fails.
        f.messaging.simulate_message(&topic(PING), request(PING)).await;
        assert!(f.messaging.replies().is_empty());
    }
}

/// Cross-language conformance against `uns-test-vectors/commands.json` (DESIGN-uns §9.5, the
/// edge-console slice S2): the inbox filter byte-for-byte, the 3 built-in verb goldens replayed
/// through a live inbox (reply bodies compared structurally — D-U22 — against the vector), the
/// `UNKNOWN_VERB` case (including the library-composed message text, which the vectors pin), and
/// the behavior flags/sets. Existence-guarded: skipped when the vectors directory is absent.
#[cfg(test)]
mod vector_tests {
    use super::*;
    use crate::testutil::RecordingMessaging;
    use serde_json::Value;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// The vectors directory, or `None` (skip) when absent.
    fn vectors_dir() -> Option<std::path::PathBuf> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../uns-test-vectors");
        if dir.is_dir() {
            Some(dir)
        } else {
            eprintln!("uns-test-vectors/ not found; skipping commands.json conformance vectors");
            None
        }
    }

    fn load(dir: &std::path::Path, file: &str) -> Value {
        let bytes = std::fs::read(dir.join(file))
            .unwrap_or_else(|e| panic!("failed to read {file}: {e}"));
        serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("{file} is not valid JSON: {e}"))
    }

    fn str_field<'a>(v: &'a Value, key: &str) -> &'a str {
        v.get(key).and_then(Value::as_str).unwrap_or_else(|| panic!("missing string '{key}' in {v}"))
    }

    /// The `gw-01`/`opcua-adapter`/`main` identity every case in `commands.json` is keyed to.
    fn vector_config() -> Arc<ArcSwap<Config>> {
        Arc::new(ArcSwap::from_pointee(
            Config::from_value("opcua-adapter", "gw-01", json!({})).unwrap(),
        ))
    }

    /// Rebuilds a vector `request` object into a live [`Message`] (pinned uuid/timestamp/
    /// correlation_id/reply_to, D-U13), ready to replay through [`RecordingMessaging`].
    fn request_message(req: &Value) -> Message {
        let header = &req["header"];
        MessageBuilder::new(str_field(header, "name"), str_field(header, "version"))
            .uuid(str_field(header, "uuid"))
            .timestamp(str_field(header, "timestamp"))
            .correlation_id(str_field(header, "correlation_id"))
            .reply_to(str_field(header, "reply_to"))
            .payload(req["body"].clone())
            .build()
    }

    #[tokio::test]
    async fn commands_json_conformance() {
        let Some(dir) = vectors_dir() else { return };
        let doc = load(&dir, "commands.json");

        // ---- inbox filter (byte-for-byte) + input echo ----
        let input = &doc["inbox"]["input"];
        assert_eq!(str_field(input, "device"), "gw-01");
        assert_eq!(str_field(input, "component"), "opcua-adapter");
        assert_eq!(str_field(input, "instance"), "main");
        assert!(!input["includeRoot"].as_bool().unwrap(), "the vectors are rootless");
        assert_eq!(str_field(input, "class"), "cmd");

        let messaging = RecordingMessaging::new();
        let config = vector_config();
        let reload_ok = Arc::new(AtomicBool::new(true));
        let reload_action: ReloadAction = {
            let reload_ok = reload_ok.clone();
            Arc::new(move || {
                let reload_ok = reload_ok.clone();
                Box::pin(async move { reload_ok.load(Ordering::SeqCst) })
            })
        };
        // The golden get-configuration reply's exact redacted config (commands.json).
        let redacted_config: Arc<dyn Fn() -> Option<Value> + Send + Sync> = Arc::new(|| {
            Some(json!({
                "component": { "name": "opcua-adapter" },
                "messaging": { "local": { "credentials": "***" } }
            }))
        });
        let inbox = CommandInbox::new(
            messaging.clone(),
            config,
            Arc::new(|| 42u64),
            reload_action,
            redacted_config,
        );
        inbox.clone().start().await;
        assert_eq!(
            messaging.subscribed_topics(),
            std::collections::HashSet::from([str_field(&doc["inbox"], "filter").to_string()]),
            "start() must subscribe exactly the vector's inbox filter"
        );

        // ---- the 3 built-in verb goldens, replayed through the live inbox ----
        let verbs = doc["verbs"].as_array().expect("verbs group");
        assert_eq!(verbs.len(), 3, "ping, reload-config, get-configuration");
        for case in verbs {
            let verb = str_field(case, "verb");
            let topic = str_field(case, "topic");
            let request = request_message(&case["request"]);
            let expected_reply_to = str_field(&case["request"]["header"], "reply_to");
            let expected_correlation_id = str_field(&case["request"]["header"], "correlation_id");

            messaging.simulate_message(topic, request).await;

            let replies = messaging.replies();
            let (reply_topic, reply) =
                replies.last().unwrap_or_else(|| panic!("verb '{verb}': no reply recorded"));
            assert_eq!(reply_topic, expected_reply_to, "verb '{verb}': reply topic mismatch");
            assert_eq!(reply.header.name, verb, "verb '{verb}': reply header.name mismatch");
            assert_eq!(reply.header.version, "1.0", "verb '{verb}': reply header.version mismatch");
            assert_eq!(
                reply.header.correlation_id, expected_correlation_id,
                "verb '{verb}': reply must carry the request's correlation_id"
            );
            assert_eq!(reply.body, case["reply"]["body"], "verb '{verb}': reply body mismatch");

            let identity = reply.identity.as_ref().unwrap_or_else(|| panic!("verb '{verb}': reply carries no identity"));
            let expected_identity = &case["reply"]["identity"];
            assert_eq!(identity.path(), str_field(expected_identity, "path"));
            assert_eq!(identity.component(), str_field(expected_identity, "component"));
            assert_eq!(identity.instance(), str_field(expected_identity, "instance"));
        }

        // ---- UNKNOWN_VERB (the library-composed message text is pinned) ----
        let errors = doc["errors"].as_array().expect("errors group");
        assert_eq!(errors.len(), 1, "unknown-verb");
        let unknown = &errors[0];
        let request = request_message(&unknown["request"]);
        messaging.simulate_message(str_field(unknown, "topic"), request).await;
        let (_, reply) = messaging.replies().last().unwrap().clone();
        assert_eq!(reply.body, unknown["reply"]["body"], "UNKNOWN_VERB reply body mismatch");

        // ---- behavior flags/sets (normative for every language's command inbox) ----
        let behavior = &doc["behavior"];
        assert!(behavior["verbIsTopicChannel"].as_bool().unwrap());
        assert!(behavior["headerNameMustEqualVerb"].as_bool().unwrap());
        assert!(behavior["fireAndForgetWithoutReplyTo"].as_bool().unwrap());
        assert!(behavior["malformedIgnoredWithoutReply"].as_bool().unwrap());
        let built_ins: Vec<&str> =
            behavior["builtInVerbs"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(built_ins, BUILT_IN_VERBS.to_vec(), "builtInVerbs");
        let delegated: Vec<&str> =
            behavior["delegatedVerbs"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(delegated, DELEGATED_VERBS.to_vec(), "delegatedVerbs");
        let error_codes: std::collections::HashSet<&str> =
            behavior["errorCodes"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(
            error_codes,
            std::collections::HashSet::from([
                ERR_UNKNOWN_VERB,
                ERR_HANDLER_ERROR,
                ERR_RELOAD_FAILED,
                ERR_NO_CONFIG
            ]),
            "errorCodes"
        );

        eprintln!(
            "uns-test-vectors commands.json: inbox filter + {} verb goldens + {} error case OK",
            verbs.len(),
            errors.len()
        );
    }
}
