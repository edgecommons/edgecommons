//! # Commands — the component command inbox + built-in verbs
//!
//! **One-liner purpose**: The library-owned component **command inbox** — the minimal
//! `commands()` facade (DESIGN-uns §7.3 / §9.5, edge-console slice S2), mirroring the Java
//! canonical `com.mbreissi.edgecommons.commands.CommandInbox`.
//!
//! ## Overview
//! Every edgecommons component subscribes, on its PRIMARY (local/IPC) connection, BOTH its
//! instance-scope and component-scope command-inbox wildcards (D-U28: the instance slot is
//! optional)
//!
//! ```text
//! ecv1/{device}/{component}/+/cmd/#     (instance-addressed)
//! ecv1/{device}/{component}/cmd/#       (component-addressed)
//! ```
//!
//! and dispatches incoming `cmd` envelopes to handlers by **verb** — the topic's channel
//! (everything after `cmd/`, `/`-namespaced verbs included), which the envelope's
//! `header.name` must equal. A request carrying `header.reply_to` gets a structured reply on
//! that topic with the request's `correlation_id` (the `uns-bridge` rewrites `reply_to` across
//! brokers, so console→component request/reply works transparently over the site bus); a `cmd`
//! without `reply_to` is fire-and-forget (the handler runs, no reply). Obtain the facade via
//! [`crate::EdgeCommons::commands`] and register custom verbs with [`CommandInbox::register`].
//!
//! ## Normative behavior (mirrored by the Java/Python/TS inboxes; pinned by
//! `uns-test-vectors/commands.json`)
//! - **Reply body shape** — success `{"ok": true, "result": <verb-specific object>}`; error
//!   `{"ok": false, "error": {"code": <CODE>, "message": <text>}}`. The reply envelope's
//!   `header.name` is the verb, `header.version` is [`CMD_MESSAGE_VERSION`], and it carries the
//!   **responder's** `identity` (and `tags`, when configured — metadata, not normative).
//! - **Built-in verbs** (registered by the library, cannot be shadowed or unregistered):
//!   [`PING`] → `{"status": "RUNNING", "uptimeSecs": n}` (liveness/echo, the state keepalive's
//!   RUNNING body shape); [`DESCRIBE`] → component command/panel discovery for edge-console;
//!   [`RELOAD_CONFIG`] → re-fetch/re-apply the configuration from the
//!   active config source (`{"reloaded": true}` or [`ERR_RELOAD_FAILED`]); [`GET_CONFIGURATION`]
//!   → return the current **redacted effective config** as `{"config": <redacted config>}` — the
//!   same redacted snapshot the `cfg` push class publishes, as a reply (**Flow B**: the console
//!   pulls a component's own config; unrelated to the Flow-A
//!   `ecv1/{device}/config/cmd/get-configuration` rendezvous where a component fetches its
//!   config FROM a config server); [`STATUS`] → [`PING`]'s per-instance superset
//!   (`{"status":"RUNNING","uptimeSecs":n[,"instances":[…]]}`), pulling the very same
//!   per-instance sample the `state` keepalive pushes
//!   ([`crate::heartbeat::Heartbeat::sample_instance_connectivity`]).
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
//! Lifecycle: constructed and [`CommandInbox::start`] by the `EdgeCommonsBuilder::build` runtime
//! right after the §9.4 [`crate::uns::RepublishListener`], whose wiring this module mirrors:
//! [`Weak`](std::sync::Weak) references in the subscribe callback so `Drop` still runs, and RAII
//! teardown (unsubscribe before the transport drops).
//!
//! ## Related Modules
//! - [`crate::uns`] — the topic builder/validator and the `_bcast` republish listener this
//!   module's wiring mirrors.
//! - [`crate::messaging`] — subscribe/reply.
//! - [`crate::config::effective`] — the redaction shared with the `cfg` push.

use std::collections::{HashMap, VecDeque};
use std::fmt::Write as _;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use arc_swap::ArcSwap;
use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::sync::{Semaphore, watch};

use crate::config::model::Config;
use crate::error::{EdgeCommonsError, Result};
use crate::heartbeat::InstanceConnectivity;
use crate::messaging::message::{Message, MessageBuilder};
use crate::messaging::{MessagingService, message_handler};
use crate::uns::{Uns, UnsClass, UnsScope};

mod deferred;
pub use deferred::{DEFERRED_REPLY_CAPACITY, DeferredReplyRegistry, DeferredReplyToken};

/// The liveness/echo built-in verb.
pub const PING: &str = "ping";
/// The component command/panel discovery built-in verb.
pub const DESCRIBE: &str = "describe";
/// The re-fetch/re-apply-configuration built-in verb.
pub const RELOAD_CONFIG: &str = "reload-config";
/// The return-my-redacted-effective-config built-in verb (Flow B).
pub const GET_CONFIGURATION: &str = "get-configuration";
/// The universal component status verb:
/// `{"status":"RUNNING","uptimeSecs":n[,"instances":[…]]}`.
///
/// [`PING`] answers only for the component as a whole. `status` is its per-instance superset: it
/// returns the same sample the `state` keepalive pushes in `instances[]`, sourced from the one
/// component-supplied [`crate::heartbeat::InstanceConnectivityProvider`] through
/// [`crate::heartbeat::Heartbeat::sample_instance_connectivity`]. Push and pull can therefore never
/// disagree — a console can subscribe, or ask, and get the same answer.
///
/// Every component implements it by registering that provider; a component with no instances (a
/// plain service) simply omits the section and answers exactly as [`PING`] does. It is deliberately
/// **not** named `sb/status`: a processor or a sink has no southbound, and this verb is universal.
pub const STATUS: &str = "status";
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
/// Error code: a deferred outcome was requested for a fire-and-forget command.
pub const ERR_REPLY_REQUIRED: &str = "REPLY_REQUIRED";
/// Error code: the inbox-owned deferred registry reached its fixed capacity.
pub const ERR_DEFERRED_CAPACITY: &str = "DEFERRED_CAPACITY";
/// Error code: a handler returned a token that was not open for this exact request.
pub const ERR_INVALID_DEFERRED_TOKEN: &str = "INVALID_DEFERRED_TOKEN";
/// Maximum inbox-owned post-accept continuations that may be running or queued.
///
/// This bound is deliberately smaller than [`DEFERRED_REPLY_CAPACITY`]: accepting a deferred
/// reply must not turn an unbounded stream of commands into an unbounded amount of application
/// work. Components should use their own durable queue for longer-lived work.
pub const POST_ACCEPT_CONTINUATION_CAPACITY: usize = 256;
/// Error code: command/deferred acceptance raced component shutdown.
pub const ERR_COMPONENT_STOPPING: &str = "COMPONENT_STOPPING";

/// The `set-config` push verb — delegated: the `CONFIG_COMPONENT` config source maintains its
/// own subscription for it on the same inbox path, so the inbox must never dispatch or
/// error-reply it.
pub const SET_CONFIG_VERB: &str = "set-config";

/// The built-in verbs (registered at construction; shadowing/unregistering is rejected). The order
/// is pinned by `uns-test-vectors/commands.json` (`behavior.builtInVerbs`).
pub const BUILT_IN_VERBS: [&str; 5] = [PING, DESCRIBE, RELOAD_CONFIG, GET_CONFIGURATION, STATUS];
/// Verbs owned by other library subscriptions on the same inbox path — always ignored.
pub const DELEGATED_VERBS: [&str; 1] = [SET_CONFIG_VERB];

/// The bounded client-side delivery queue for the inbox's single wildcard subscription.
const SUBSCRIBE_MAX_MESSAGES: usize = MAX_PENDING_STARTUP_DELIVERIES;
/// Serial delivery preserves broker arrival order through the STARTING -> ACTIVE gate.
const SUBSCRIBE_MAX_CONCURRENCY: usize = 1;
/// Default bounded acknowledgement deadline for command-inbox startup.
pub const DEFAULT_START_TIMEOUT: Duration = Duration::from_secs(10);
/// Strict maximum retained between subscription acknowledgement and ACTIVE dispatch.
pub const MAX_PENDING_STARTUP_DELIVERIES: usize = 256;
const MAX_START_ERROR_CHARS: usize = 256;

/// Observable command-plane lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandInboxStartupState {
    /// One acknowledged-subscribe attempt is in progress.
    Starting,
    /// Handlers are installed and the transport has acknowledged the subscription.
    Active,
    /// Startup failed; `error` contains a bounded sanitized diagnostic.
    Failed,
    /// No active start generation exists.
    Stopped,
}

/// Immutable lifecycle status returned by startup and queried by readiness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandInboxStartupStatus {
    /// Current lifecycle state.
    pub state: CommandInboxStartupState,
    /// Sanitized stable startup error; empty outside `Failed`.
    pub error: String,
}

/// Internal observer used to make runtime readiness follow command-plane lifecycle.
pub(crate) type StartupObserver = Arc<dyn Fn(CommandInboxStartupState) + Send + Sync>;

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
        Self {
            code: code.into(),
            message: message.into(),
        }
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
/// use edgecommons::commands::command_handler;
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

/// The explicit outcome of a long- or short-running command handler.
///
/// This is parallel to, and does not change, the legacy [`CommandHandler`] result contract.
#[non_exhaustive]
pub enum CommandOutcome {
    /// Send the ordinary success wrapper immediately (`None` means an empty result object).
    ImmediateSuccess(Option<Value>),
    /// Send the ordinary coded error wrapper immediately.
    ImmediateError(CommandError),
    /// Suppress automatic reply; the already-activated opaque token will settle it later.
    Deferred(DeferredReplyToken),
    /// Suppress the automatic reply and run `continuation` only after the inbox has accepted the
    /// already-open token for this exact request.
    ///
    /// The inbox owns scheduling the continuation, so application code cannot race token
    /// validation by spawning work before returning from the handler. The continuation must
    /// settle the captured token (or arrange durable work that does so) through the normal
    /// [`DeferredReplyToken`] API. Returning a [`CommandError`] produces one guarded error
    /// settlement rather than leaving an open token to expire.
    DeferredWithContinuation {
        /// The activated reply token returned by the handler.
        token: DeferredReplyToken,
        /// Work to begin after the inbox accepts `token`.
        continuation: PostAcceptContinuation,
    },
}

impl CommandOutcome {
    /// Builds a deferred outcome whose continuation begins only after inbox acceptance.
    ///
    /// This is the race-free construction path for a command that durably accepts work and then
    /// needs to start an asynchronous operation. `token` must already be activated.
    pub fn deferred_with_continuation<F>(token: DeferredReplyToken, continuation: F) -> Self
    where
        F: Future<Output = std::result::Result<(), CommandError>> + Send + 'static,
    {
        Self::DeferredWithContinuation {
            token,
            continuation: Box::pin(continuation),
        }
    }
}

impl std::fmt::Debug for CommandOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ImmediateSuccess(result) => {
                f.debug_tuple("ImmediateSuccess").field(result).finish()
            }
            Self::ImmediateError(error) => f.debug_tuple("ImmediateError").field(error).finish(),
            Self::Deferred(token) => f.debug_tuple("Deferred").field(token).finish(),
            Self::DeferredWithContinuation { token, .. } => f
                .debug_struct("DeferredWithContinuation")
                .field("token", token)
                .finish_non_exhaustive(),
        }
    }
}

/// A bounded, inbox-owned asynchronous action that starts after a deferred token is accepted.
pub type PostAcceptContinuation =
    Pin<Box<dyn Future<Output = std::result::Result<(), CommandError>> + Send + 'static>>;

/// A command handler that can explicitly defer its reply.
///
/// The inbox supplies a clone of its bounded [`DeferredReplyRegistry`] for provisional token
/// creation. The handler must durably accept its application work, call
/// [`DeferredReplyToken::activate`], then return [`CommandOutcome::Deferred`].
#[async_trait]
pub trait OutcomeCommandHandler: Send + Sync + 'static {
    /// Handle one request and select immediate or deferred settlement.
    async fn handle(&self, request: Message, deferred: DeferredReplyRegistry) -> CommandOutcome;
}

/// Adapts an async closure into an [`OutcomeCommandHandler`].
struct FnOutcomeCommandHandler<F>(F);

#[async_trait]
impl<F, Fut> OutcomeCommandHandler for FnOutcomeCommandHandler<F>
where
    F: Fn(Message, DeferredReplyRegistry) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = CommandOutcome> + Send + 'static,
{
    async fn handle(&self, request: Message, deferred: DeferredReplyRegistry) -> CommandOutcome {
        (self.0)(request, deferred).await
    }
}

/// Wrap an async closure for [`CommandInbox::register_outcome`].
pub fn outcome_handler<F, Fut>(f: F) -> Arc<dyn OutcomeCommandHandler>
where
    F: Fn(Message, DeferredReplyRegistry) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = CommandOutcome> + Send + 'static,
{
    Arc::new(FnOutcomeCommandHandler(f))
}

/// One out-of-band re-fetch-and-apply action (the [`RELOAD_CONFIG`] verb's action): re-invokes
/// the active config source's `load()`, validates, and — only on success — atomically applies the
/// new snapshot and notifies listeners (production: [`crate::apply_reloaded_config`] over the
/// source captured at build time). Returns `true` on success. An infallible, best-effort async
/// callback — failures are logged internally, mirroring [`crate::uns::RepublishAction`].
pub(crate) type ReloadAction =
    Arc<dyn Fn() -> Pin<Box<dyn Future<Output = bool> + Send>> + Send + Sync>;

/// The [`STATUS`] verb's source: one sample of the component's per-instance connectivity
/// (production: [`crate::heartbeat::Heartbeat::sample_instance_connectivity`], i.e. the very same
/// provider the `state` keepalive pushes, so the pulled answer and the pushed one cannot diverge).
/// Best-effort by contract — it never fails; an empty vec omits the reply's `instances[]` section.
pub(crate) type InstanceConnectivitySource =
    Arc<dyn Fn() -> Vec<InstanceConnectivity> + Send + Sync>;

/// Lifecycle flags + the resolved inbox topic, behind one lock (no `.await` ever happens while
/// holding it) — mirrors [`crate::uns::RepublishListener`]'s `Inner`.
struct Inner {
    closed: bool,
    /// The instance-scope inbox filter (`…/{instance|+}/cmd/#`); `None` until
    /// [`CommandInbox::start`] builds it. D-U28: with a component-scope identity the instance
    /// slot renders `+`, matching any instance-addressed command.
    inbox_filter: Option<String>,
    /// The component-scope inbox filter (`…/cmd/#`, D-U28); `None` until [`CommandInbox::start`]
    /// builds it. Subscribed alongside [`Self::inbox_filter`] so component-scope commands land too.
    component_inbox_filter: Option<String>,
    startup_state: CommandInboxStartupState,
    startup_error: String,
    startup_generation: u64,
    pending: VecDeque<(String, Message)>,
    retained: usize,
    draining: bool,
}

impl Default for Inner {
    fn default() -> Self {
        Self {
            closed: false,
            inbox_filter: None,
            component_inbox_filter: None,
            startup_state: CommandInboxStartupState::Stopped,
            startup_error: String::new(),
            startup_generation: 0,
            pending: VecDeque::new(),
            retained: 0,
            draining: false,
        }
    }
}

/// The library-owned component **command inbox** — see the [module docs](self).
pub struct CommandInbox {
    messaging: Arc<dyn MessagingService>,
    config: Arc<ArcSwap<Config>>,
    /// verb → handler; built-ins seeded at construction, custom verbs via [`Self::register`].
    handlers: Mutex<HashMap<String, Arc<dyn CommandHandler>>>,
    /// verb → explicit outcome handler; kept separate so the legacy handler trait is untouched.
    outcome_handlers: Mutex<HashMap<String, Arc<dyn OutcomeCommandHandler>>>,
    /// Serializes cross-map registration/unregistration so one verb can never enter both maps.
    registration: Mutex<()>,
    /// Component-provided edge-console panel descriptors, registered via [`Self::register_panel`].
    panels: Mutex<Vec<Value>>,
    deferred: DeferredReplyRegistry,
    /// Capacity for inbox-owned continuations that start only after deferred acceptance.
    post_accept_capacity: Arc<Semaphore>,
    /// Broadcast cancellation for owned continuations during terminal inbox shutdown.
    post_accept_shutdown: watch::Sender<bool>,
    startup_observer: StartupObserver,
    inner: Mutex<Inner>,
}

impl CommandInbox {
    /// Creates the inbox and registers the built-in verbs. The verb *actions* are injected
    /// seams so the built-ins unit-test deterministically; `EdgeCommonsBuilder::build` wires the
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
    /// - `instance_connectivity` — the [`STATUS`] source (see [`InstanceConnectivitySource`]).
    #[cfg(test)]
    pub(crate) fn new(
        messaging: Arc<dyn MessagingService>,
        config: Arc<ArcSwap<Config>>,
        uptime_secs: Arc<dyn Fn() -> u64 + Send + Sync>,
        reload_action: ReloadAction,
        redacted_config: Arc<dyn Fn() -> Option<Value> + Send + Sync>,
        instance_connectivity: InstanceConnectivitySource,
    ) -> Arc<CommandInbox> {
        Self::new_with_startup_observer(
            messaging,
            config,
            uptime_secs,
            reload_action,
            redacted_config,
            instance_connectivity,
            Arc::new(|_| {}),
        )
    }

    pub(crate) fn new_with_startup_observer(
        messaging: Arc<dyn MessagingService>,
        config: Arc<ArcSwap<Config>>,
        uptime_secs: Arc<dyn Fn() -> u64 + Send + Sync>,
        reload_action: ReloadAction,
        redacted_config: Arc<dyn Fn() -> Option<Value> + Send + Sync>,
        instance_connectivity: InstanceConnectivitySource,
        startup_observer: StartupObserver,
    ) -> Arc<CommandInbox> {
        let mut handlers: HashMap<String, Arc<dyn CommandHandler>> = HashMap::new();

        // ping -> the state keepalive's RUNNING body shape: proves the component is not just
        // alive (the keepalive does that) but RESPONSIVE to addressed commands.
        let ping_uptime_secs = uptime_secs.clone();
        handlers.insert(
            PING.to_string(),
            command_handler(move |_request| {
                let uptime_secs = ping_uptime_secs.clone();
                async move {
                    Ok(Some(
                        json!({ "status": "RUNNING", "uptimeSecs": (uptime_secs)() }),
                    ))
                }
            }),
        );

        // status -> ping's per-instance superset. Same body, plus the instances[] the state
        // keepalive pushes, from the SAME provider. A component with no instances omits the
        // section, so a plain service answers exactly as ping does.
        handlers.insert(
            STATUS.to_string(),
            command_handler(move |_request| {
                let uptime_secs = uptime_secs.clone();
                let instance_connectivity = instance_connectivity.clone();
                async move {
                    let mut result = serde_json::Map::new();
                    result.insert("status".to_string(), json!("RUNNING"));
                    result.insert("uptimeSecs".to_string(), json!((uptime_secs)()));
                    let instances = (instance_connectivity)();
                    if !instances.is_empty() {
                        result.insert(
                            "instances".to_string(),
                            Value::Array(
                                instances
                                    .iter()
                                    .map(InstanceConnectivity::to_json)
                                    .collect(),
                            ),
                        );
                    }
                    Ok(Some(Value::Object(result)))
                }
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

        let deferred = DeferredReplyRegistry::new(messaging.clone(), config.clone());
        let (post_accept_shutdown, _) = watch::channel(false);
        let inbox = Arc::new(CommandInbox {
            messaging,
            config,
            handlers: Mutex::new(handlers),
            outcome_handlers: Mutex::new(HashMap::new()),
            registration: Mutex::new(()),
            panels: Mutex::new(Vec::new()),
            deferred,
            post_accept_capacity: Arc::new(Semaphore::new(POST_ACCEPT_CONTINUATION_CAPACITY)),
            post_accept_shutdown,
            startup_observer,
            inner: Mutex::new(Inner::default()),
        });

        let weak = Arc::downgrade(&inbox);
        inbox.handlers.lock().unwrap().insert(
            DESCRIBE.to_string(),
            command_handler(move |_request| {
                let weak = weak.clone();
                async move {
                    Ok(Some(match weak.upgrade() {
                        Some(inbox) => inbox.describe(),
                        None => describe_payload(Vec::new(), Vec::new(), None),
                    }))
                }
            }),
        );

        inbox
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
    /// [`EdgeCommonsError::UnsValidation`] when a verb token violates the §2.2 token rule;
    /// [`EdgeCommonsError::Command`] when the verb is built-in/delegated/already registered.
    pub fn register(&self, verb: &str, handler: Arc<dyn CommandHandler>) -> Result<()> {
        for token in verb.split('/') {
            crate::uns::check_token(token, "verb token")?;
        }
        if BUILT_IN_VERBS.contains(&verb) {
            return Err(EdgeCommonsError::Command(format!(
                "verb '{verb}' is a built-in verb and cannot be shadowed"
            )));
        }
        if DELEGATED_VERBS.contains(&verb) {
            return Err(EdgeCommonsError::Command(format!(
                "verb '{verb}' is owned by another library subsystem and cannot be registered"
            )));
        }
        let _registration = self.registration.lock().map_err(|_| {
            EdgeCommonsError::Command("command registration lock is poisoned".to_string())
        })?;
        let mut handlers = self.handlers.lock().unwrap();
        if handlers.contains_key(verb) || self.outcome_handlers.lock().unwrap().contains_key(verb) {
            return Err(EdgeCommonsError::Command(format!(
                "verb '{verb}' is already registered - unregister it first to replace the handler"
            )));
        }
        handlers.insert(verb.to_string(), handler);
        tracing::debug!(verb, "command verb registered");
        Ok(())
    }

    /// Registers an explicit immediate/deferred outcome handler.
    ///
    /// Validation, namespaced verbs, precedence, and replacement rules are identical to
    /// [`Self::register`]. The legacy [`CommandHandler`] path remains unchanged and separate.
    pub fn register_outcome(
        &self,
        verb: &str,
        handler: Arc<dyn OutcomeCommandHandler>,
    ) -> Result<()> {
        for token in verb.split('/') {
            crate::uns::check_token(token, "verb token")?;
        }
        if BUILT_IN_VERBS.contains(&verb) {
            return Err(EdgeCommonsError::Command(format!(
                "verb '{verb}' is a built-in verb and cannot be shadowed"
            )));
        }
        if DELEGATED_VERBS.contains(&verb) {
            return Err(EdgeCommonsError::Command(format!(
                "verb '{verb}' is owned by another library subsystem and cannot be registered"
            )));
        }
        let _registration = self.registration.lock().map_err(|_| {
            EdgeCommonsError::Command("command registration lock is poisoned".to_string())
        })?;
        let mut outcome_handlers = self.outcome_handlers.lock().unwrap();
        if outcome_handlers.contains_key(verb) || self.handlers.lock().unwrap().contains_key(verb) {
            return Err(EdgeCommonsError::Command(format!(
                "verb '{verb}' is already registered - unregister it first to replace the handler"
            )));
        }
        outcome_handlers.insert(verb.to_string(), handler);
        tracing::debug!(verb, "outcome command verb registered");
        Ok(())
    }

    /// A clone of this inbox's bounded deferred-reply registry.
    pub fn deferred_replies(&self) -> DeferredReplyRegistry {
        self.deferred.clone()
    }

    /// Create a provisional deferred token owned by this inbox.
    pub fn defer(
        &self,
        request: &Message,
        lifetime: std::time::Duration,
    ) -> std::result::Result<DeferredReplyToken, CommandError> {
        self.deferred.defer(request, lifetime)
    }

    /// Removes a previously registered custom verb handler. Unknown verbs are a no-op; built-in
    /// verbs cannot be unregistered.
    ///
    /// # Errors
    /// [`EdgeCommonsError::Command`] when `verb` is a built-in.
    pub fn unregister(&self, verb: &str) -> Result<()> {
        if BUILT_IN_VERBS.contains(&verb) {
            return Err(EdgeCommonsError::Command(format!(
                "verb '{verb}' is a built-in verb and cannot be unregistered"
            )));
        }
        let _registration = self.registration.lock().map_err(|_| {
            EdgeCommonsError::Command("command registration lock is poisoned".to_string())
        })?;
        let removed = self.handlers.lock().unwrap().remove(verb).is_some()
            || self.outcome_handlers.lock().unwrap().remove(verb).is_some();
        if removed {
            tracing::debug!(verb, "command verb unregistered");
        }
        Ok(())
    }

    /// The currently registered verbs (built-ins + custom) — a snapshot copy.
    pub fn verbs(&self) -> std::collections::HashSet<String> {
        let mut verbs: std::collections::HashSet<String> =
            self.handlers.lock().unwrap().keys().cloned().collect();
        verbs.extend(self.outcome_handlers.lock().unwrap().keys().cloned());
        verbs
    }

    /// Starts a continuation only after its token has been accepted for a specific command.
    ///
    /// The caller has already checked token ownership, request metadata, and `OPEN` state. A
    /// full continuation queue settles the otherwise-valid token with a standard error on a
    /// detached task; it never blocks command dispatch or leaves a silent open reply.
    fn start_post_accept_continuation(
        &self,
        token: DeferredReplyToken,
        continuation: PostAcceptContinuation,
    ) {
        let Ok(permit) = self.post_accept_capacity.clone().try_acquire_owned() else {
            tracing::warn!("post-accept deferred continuation capacity exhausted");
            tokio::spawn(async move {
                if let Err(error) = token
                    .settle_error(
                        ERR_HANDLER_ERROR,
                        "the deferred command continuation could not be started",
                    )
                    .await
                {
                    tracing::debug!(error = %error, "could not settle rejected deferred continuation");
                }
            });
            return;
        };

        let mut shutdown = self.post_accept_shutdown.subscribe();
        let settlement = token.clone();
        tokio::spawn(async move {
            let _permit = permit;
            if *shutdown.borrow() {
                return;
            }
            tokio::select! {
                result = continuation => {
                    if let Err(error) = result {
                        if let Err(settlement_error) = settlement.settle_command_error(error).await {
                            tracing::debug!(error = %settlement_error, "could not settle failed post-accept continuation");
                        }
                    }
                },
                _ = shutdown.changed() => {},
            }
        });
    }

    /// Registers a component-provided edge-console panel descriptor for [`DESCRIBE`].
    ///
    /// The core library validates only the portable discovery contract: `panel` must be a JSON
    /// object with non-empty string `id` and `title` fields, and `id` must be unique. All other
    /// descriptor fields are carried through unchanged for the console-owned renderer.
    ///
    /// # Errors
    /// [`EdgeCommonsError::Command`] when the panel is not an object, `id`/`title` is missing or
    /// empty, or another registered panel already uses the same `id`.
    pub fn register_panel(&self, panel: Value) -> Result<()> {
        let object = panel.as_object().ok_or_else(|| {
            EdgeCommonsError::Command("panel descriptor must be a JSON object".to_string())
        })?;
        let id = object
            .get("id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                EdgeCommonsError::Command(
                    "panel descriptor field 'id' must be a non-empty string".to_string(),
                )
            })?
            .to_string();
        object
            .get("title")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                EdgeCommonsError::Command(
                    "panel descriptor field 'title' must be a non-empty string".to_string(),
                )
            })?;

        let mut panels = self.panels.lock().unwrap();
        if panels
            .iter()
            .any(|p| p.get("id").and_then(Value::as_str) == Some(id.as_str()))
        {
            return Err(EdgeCommonsError::Command(format!(
                "panel id '{id}' is already registered"
            )));
        }
        panels.push(panel);
        Ok(())
    }

    /// The currently registered panel descriptors — a snapshot copy.
    pub fn panels(&self) -> Vec<Value> {
        self.panels.lock().unwrap().clone()
    }

    fn describe(&self) -> Value {
        let mut verbs: Vec<String> = self.verbs().into_iter().collect();
        verbs.sort();
        let commands = verbs
            .into_iter()
            .map(|verb| {
                let built_in = BUILT_IN_VERBS.contains(&verb.as_str());
                json!({ "verb": verb, "builtIn": built_in })
            })
            .collect();
        let snapshot = self.config.load_full();
        let identity = snapshot.identity();
        let mut component = serde_json::Map::new();
        component.insert(
            "hier".to_string(),
            Value::Array(
                identity
                    .hier()
                    .iter()
                    .map(|entry| json!({ "level": entry.level, "value": entry.value }))
                    .collect(),
            ),
        );
        component.insert("path".to_string(), json!(identity.path()));
        component.insert("component".to_string(), json!(identity.component()));
        // D-U28: the `instance` key is present only for an instance-scoped identity - a
        // component-scope identity omits it (mirroring the envelope identity serialization).
        if let Some(instance) = identity.instance() {
            component.insert("instance".to_string(), json!(instance));
        }
        describe_payload(commands, self.panels(), Some(Value::Object(component)))
    }

    /// Start with the default bounded acknowledgement deadline.
    pub async fn start(self: Arc<Self>) -> CommandInboxStartupStatus {
        self.start_with_timeout(DEFAULT_START_TIMEOUT).await
    }

    /// Start one retryable lifecycle generation. `Active` is published only after strict
    /// transport acknowledgement and while the bounded activation gate can accept work.
    pub async fn start_with_timeout(
        self: Arc<Self>,
        timeout: Duration,
    ) -> CommandInboxStartupStatus {
        if timeout.is_zero() {
            let mut inner = self.inner.lock().unwrap();
            inner.startup_state = CommandInboxStartupState::Failed;
            inner.startup_error = "command inbox start timeout must be positive".to_string();
            (self.startup_observer)(inner.startup_state);
            return Self::status_locked(&inner);
        }

        let generation = {
            let mut inner = self.inner.lock().unwrap();
            if inner.closed {
                return CommandInboxStartupStatus {
                    state: CommandInboxStartupState::Stopped,
                    error: "command inbox is closed".to_string(),
                };
            }
            if matches!(
                inner.startup_state,
                CommandInboxStartupState::Starting | CommandInboxStartupState::Active
            ) {
                return Self::status_locked(&inner);
            }
            inner.startup_generation += 1;
            inner.startup_state = CommandInboxStartupState::Starting;
            inner.startup_error.clear();
            inner.pending.clear();
            inner.retained = 0;
            inner.draining = false;
            (self.startup_observer)(inner.startup_state);
            inner.startup_generation
        };

        let snapshot = self.config.load_full();
        let identity = snapshot.identity();
        let uns = Uns::new(identity.clone(), snapshot.topic_include_root());
        let site = if identity.hier().len() >= 2 {
            Some(identity.hier()[0].value.clone())
        } else {
            None
        };
        // D-U28: the component identity is component scope (instance None), so a plain filter
        // renders the instance slot as `+` (instance-scoped: `.../+/cmd/#`); the component-scope
        // filter omits the instance slot (`.../cmd/#`). Subscribe both so a command lands whether
        // it is addressed to an instance or to the component.
        let scope = UnsScope {
            site,
            device: Some(identity.device().to_string()),
            component: Some(identity.component().to_string()),
            instance: identity.instance().map(str::to_string),
        };
        let build_filter = |include_instance: bool| {
            uns.filter_scoped(UnsClass::Cmd, &scope, include_instance)
        };
        let (filter, component_filter) = match (build_filter(true), build_filter(false)) {
            (Ok(filter), Ok(component_filter)) => (filter, component_filter),
            (Err(error), _) | (_, Err(error)) => {
                self.fail_start(generation, &error.to_string());
                return self.startup_status();
            }
        };
        {
            let mut inner = self.inner.lock().unwrap();
            if inner.closed
                || inner.startup_generation != generation
                || inner.startup_state != CommandInboxStartupState::Starting
            {
                return Self::status_locked(&inner);
            }
            inner.inbox_filter = Some(filter.clone());
            inner.component_inbox_filter = Some(component_filter.clone());
        }

        let make_handler = || {
            let weak = Arc::downgrade(&self);
            message_handler(move |topic, message| {
                let weak = weak.clone();
                async move {
                    if let Some(inbox) = weak.upgrade() {
                        inbox
                            .receive_during_activation(generation, topic, message)
                            .await;
                    }
                }
            })
        };
        for subscribe_filter in [&filter, &component_filter] {
            if let Err(error) = self
                .messaging
                .subscribe_acknowledged(
                    subscribe_filter,
                    make_handler(),
                    SUBSCRIBE_MAX_MESSAGES,
                    SUBSCRIBE_MAX_CONCURRENCY,
                    timeout,
                )
                .await
            {
                let _ = self.messaging.unsubscribe(&filter).await;
                let _ = self.messaging.unsubscribe(&component_filter).await;
                self.fail_start(generation, &error.to_string());
                tracing::warn!(error = %error, filter, component_filter, "command inbox startup failed");
                return self.startup_status();
            }
            // If this generation was invalidated mid-ack (e.g. stop() raced the second
            // acknowledged-subscribe), tear down what we have and stop - never leave a partial
            // subscription live for a stale generation.
            let still_current = {
                let inner = self.inner.lock().unwrap();
                !inner.closed
                    && inner.startup_generation == generation
                    && inner.startup_state == CommandInboxStartupState::Starting
            };
            if !still_current {
                let _ = self.messaging.unsubscribe(&filter).await;
                let _ = self.messaging.unsubscribe(&component_filter).await;
                return self.startup_status();
            }
        }

        let should_drain = {
            let mut inner = self.inner.lock().unwrap();
            if inner.closed
                || inner.startup_generation != generation
                || inner.startup_state != CommandInboxStartupState::Starting
            {
                false
            } else {
                inner.startup_state = CommandInboxStartupState::Active;
                inner.startup_error.clear();
                inner.draining = !inner.pending.is_empty();
                (self.startup_observer)(inner.startup_state);
                inner.draining
            }
        };

        if self.startup_status().state != CommandInboxStartupState::Active {
            let _ = self.messaging.unsubscribe(&filter).await;
            let _ = self.messaging.unsubscribe(&component_filter).await;
            return self.startup_status();
        }
        if should_drain {
            let inbox = self.clone();
            tokio::spawn(async move { inbox.drain_activation_gate(generation).await });
        }
        tracing::info!(filter, component_filter, verbs = ?self.verbs(), "command inbox subscribed");
        self.startup_status()
    }

    /// Current immutable lifecycle status.
    pub fn startup_status(&self) -> CommandInboxStartupStatus {
        let inner = self.inner.lock().unwrap();
        Self::status_locked(&inner)
    }

    /// Stop the current generation and clean its subscriptions. A later start may retry.
    pub async fn stop(&self) -> CommandInboxStartupStatus {
        let filters = {
            let mut inner = self.inner.lock().unwrap();
            inner.startup_generation += 1;
            inner.startup_state = CommandInboxStartupState::Stopped;
            inner.startup_error.clear();
            inner.pending.clear();
            inner.retained = 0;
            inner.draining = false;
            let filters = [
                inner.inbox_filter.take(),
                inner.component_inbox_filter.take(),
            ];
            (self.startup_observer)(inner.startup_state);
            filters
        };
        for filter in filters.into_iter().flatten() {
            let _ = self.messaging.unsubscribe(&filter).await;
        }
        self.startup_status()
    }

    fn status_locked(inner: &Inner) -> CommandInboxStartupStatus {
        CommandInboxStartupStatus {
            state: inner.startup_state,
            error: inner.startup_error.clone(),
        }
    }

    fn fail_start(&self, generation: u64, error: &str) {
        let mut inner = self.inner.lock().unwrap();
        if inner.startup_generation != generation
            || inner.startup_state != CommandInboxStartupState::Starting
        {
            return;
        }
        inner.startup_state = CommandInboxStartupState::Failed;
        inner.startup_error = sanitize_start_error(error);
        inner.inbox_filter = None;
        inner.component_inbox_filter = None;
        inner.pending.clear();
        inner.retained = 0;
        inner.draining = false;
        (self.startup_observer)(inner.startup_state);
    }

    async fn receive_during_activation(
        self: Arc<Self>,
        generation: u64,
        topic: String,
        message: Message,
    ) {
        let mut delivery = Some((topic, message));
        let dispatch_now = {
            let mut inner = self.inner.lock().unwrap();
            if inner.closed || inner.startup_generation != generation {
                false
            } else if inner.startup_state == CommandInboxStartupState::Starting
                || (inner.startup_state == CommandInboxStartupState::Active && inner.draining)
            {
                if inner.retained >= MAX_PENDING_STARTUP_DELIVERIES {
                    tracing::warn!(
                        capacity = MAX_PENDING_STARTUP_DELIVERIES,
                        "dropping command delivery because the startup activation gate is full"
                    );
                } else {
                    inner.pending.push_back(delivery.take().unwrap());
                    inner.retained += 1;
                }
                false
            } else {
                inner.startup_state == CommandInboxStartupState::Active
            }
        };
        if dispatch_now {
            let (topic, message) = delivery.unwrap();
            self.dispatch_delivery(generation, topic, message).await;
        }
    }

    async fn drain_activation_gate(self: Arc<Self>, generation: u64) {
        loop {
            let next = {
                let mut inner = self.inner.lock().unwrap();
                if inner.closed
                    || inner.startup_generation != generation
                    || inner.startup_state != CommandInboxStartupState::Active
                {
                    inner.pending.clear();
                    inner.retained = 0;
                    inner.draining = false;
                    return;
                }
                match inner.pending.pop_front() {
                    Some(delivery) => Some(delivery),
                    None => {
                        inner.retained = 0;
                        inner.draining = false;
                        None
                    }
                }
            };
            let Some((topic, message)) = next else {
                return;
            };
            self.clone()
                .dispatch_delivery(generation, topic, message)
                .await;
            let mut inner = self.inner.lock().unwrap();
            if inner.startup_generation == generation && inner.retained > 0 {
                inner.retained -= 1;
            }
        }
    }

    /// One received `cmd` envelope after lifecycle gating.
    async fn dispatch_delivery(self: Arc<Self>, generation: u64, topic: String, message: Message) {
        {
            let inner = self.inner.lock().unwrap();
            if inner.closed
                || inner.startup_generation != generation
                || inner.startup_state != CommandInboxStartupState::Active
            {
                return;
            }
        }
        // D-U28: the instance slot is optional, so a command arrives on either
        // ".../{instance}/cmd/{verb}" or ".../cmd/{verb}". Locate the "/cmd/" class marker and
        // take the verb after it - unambiguous for both scopes (an instance is never a class
        // token). Both subscribed filters share this extraction path.
        let Some(marker) = topic.find("/cmd/") else {
            // ".../cmd/#" also matches the bare ".../cmd" parent level - nothing to dispatch.
            tracing::debug!(topic = %topic, "ignoring cmd delivery without a '/cmd/' segment");
            return;
        };
        let verb = &topic[marker + "/cmd/".len()..];
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
        Self::dispatch(self, verb.to_string(), message).await;
    }

    /// Dispatches a well-formed request to its handler and replies (when `reply_to` is set).
    async fn dispatch(inbox: Arc<Self>, verb: String, request: Message) {
        let wants_reply = request
            .header
            .reply_to
            .as_deref()
            .is_some_and(|s| !s.is_empty());
        let handler = { inbox.handlers.lock().unwrap().get(&verb).cloned() };
        let outcome_handler = { inbox.outcome_handlers.lock().unwrap().get(&verb).cloned() };
        if handler.is_none() && outcome_handler.is_none() {
            if wants_reply {
                tracing::debug!(
                    verb,
                    code = ERR_UNKNOWN_VERB,
                    "unknown verb; sending error reply"
                );
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
        }

        if let Some(handler) = handler {
            // Legacy path, intentionally unchanged: one clone for handler ownership, immediate
            // best-effort reply, and fire-and-forget result discard.
            match handler.handle(request.clone()).await {
                Ok(result) => {
                    if wants_reply {
                        let body =
                            json!({ "ok": true, "result": result.unwrap_or_else(|| json!({})) });
                        inbox.send_reply(&request, &verb, body).await;
                    }
                }
                Err(e) => {
                    if wants_reply {
                        inbox
                            .send_reply(&request, &verb, error_body(&e.code, e.message))
                            .await;
                    } else {
                        tracing::warn!(verb, code = %e.code, message = %e.message, "fire-and-forget verb failed");
                    }
                }
            }
            return;
        }

        let Some(outcome_handler) = outcome_handler else {
            return;
        };
        let outcome = outcome_handler
            .handle(request.clone(), inbox.deferred.clone())
            .await;
        match outcome {
            CommandOutcome::ImmediateSuccess(result) => {
                if wants_reply {
                    let body = json!({ "ok": true, "result": result.unwrap_or_else(|| json!({})) });
                    inbox.send_reply(&request, &verb, body).await;
                }
            }
            CommandOutcome::ImmediateError(error) => {
                if wants_reply {
                    inbox
                        .send_reply(&request, &verb, error_body(&error.code, error.message))
                        .await;
                } else {
                    tracing::warn!(verb, code = %error.code, message = %error.message, "fire-and-forget outcome verb failed");
                }
            }
            CommandOutcome::Deferred(token) => {
                if inbox
                    .accept_deferred_token(&token, &request, &verb, wants_reply)
                    .await
                {
                    tracing::debug!(verb, "command reply deferred; dispatcher permit released");
                }
            }
            CommandOutcome::DeferredWithContinuation {
                token,
                continuation,
            } => {
                if inbox
                    .accept_deferred_token(&token, &request, &verb, wants_reply)
                    .await
                {
                    // `accept_deferred_token` proved this exact token OPEN before any application
                    // continuation can run. Scheduling is inbox-owned to close the handler-return
                    // race while still releasing the normal dispatch permit immediately.
                    inbox.start_post_accept_continuation(token, continuation);
                    tracing::debug!(
                        verb,
                        "command reply deferred; post-accept continuation scheduled"
                    );
                }
            }
        }
    }

    /// Validates a returned deferred token and emits the standard invalid-outcome reply on error.
    ///
    /// `true` means the token was `OPEN` and belonged to this inbox, request, and verb at the
    /// point of acceptance. Callers must not start application continuations unless this returns
    /// `true`.
    async fn accept_deferred_token(
        &self,
        token: &DeferredReplyToken,
        request: &Message,
        verb: &str,
        wants_reply: bool,
    ) -> bool {
        if let Err(error) = self.deferred.validate_open_token(token, request) {
            // A same-request provisional token is safely discarded; an OPEN token for a
            // different request is left untouched rather than cancelling unrelated work.
            self.deferred.discard_provisional_token_for(token, request);
            tracing::warn!(verb, error = %error, "handler returned an invalid deferred token");
            if wants_reply {
                self.send_reply(
                    request,
                    verb,
                    error_body(ERR_INVALID_DEFERRED_TOKEN, error.to_string()),
                )
                .await;
            }
            false
        } else {
            true
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
            .command(body)
            .from_config(&snapshot)
            .build();
        if let Err(e) = self.messaging.reply(request, reply).await {
            tracing::warn!(error = %e, verb, "command reply failed");
        }
    }

    /// Marks the inbox closed (idempotent) and returns the filters to unsubscribe (empty if
    /// already closed or never started). Shared by [`Self::close`] and [`Drop::drop`]. D-U28:
    /// both the instance- and component-scope filters.
    fn mark_closed(&self) -> Vec<String> {
        let mut inner = self.inner.lock().unwrap();
        if inner.closed {
            return Vec::new();
        }
        inner.closed = true;
        inner.startup_generation += 1;
        inner.startup_state = CommandInboxStartupState::Stopped;
        inner.startup_error.clear();
        inner.pending.clear();
        inner.retained = 0;
        inner.draining = false;
        let filters = [
            inner.inbox_filter.take(),
            inner.component_inbox_filter.take(),
        ];
        (self.startup_observer)(inner.startup_state);
        filters.into_iter().flatten().collect()
    }

    /// Stop deferred acceptance, attempt `COMPONENT_STOPPING` replies for open tokens, and cancel
    /// the remaining registry entries. Idempotent.
    ///
    /// Applications with an explicit shutdown sequence should await this before dropping the
    /// messaging transport. [`Drop`] also schedules the same bounded cleanup as a fallback.
    pub async fn shutdown_deferred(&self) {
        self.post_accept_shutdown.send_replace(true);
        self.deferred.shutdown().await;
    }

    /// Test-only deterministic teardown: the same unsubscribe-before-exit logic as
    /// [`Drop::drop`], but awaited synchronously (no fire-and-forget spawn), so tests can assert
    /// the post-close state without polling/sleeping. Idempotent. Production teardown is
    /// RAII-only (`Drop`, mirroring [`crate::uns::RepublishListener`]) — this is not part of the
    /// production wiring, hence `#[cfg(test)]`.
    #[cfg(test)]
    pub(crate) async fn close(&self) {
        self.shutdown_deferred().await;
        for filter in self.mark_closed() {
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
        let filters = self.mark_closed();
        let messaging = self.messaging.clone();
        let deferred = self.deferred.clone();
        self.post_accept_shutdown.send_replace(true);
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                deferred.shutdown().await;
                for filter in filters {
                    if let Err(e) = messaging.unsubscribe(&filter).await {
                        tracing::debug!(error = %e, filter, "command-inbox unsubscribe failed");
                    }
                }
            });
        }
    }
}

fn sanitize_start_error(error: &str) -> String {
    let safe: String = error
        .chars()
        .take(MAX_START_ERROR_CHARS)
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let redacted = regex::Regex::new(r"(?i)(password|passwd|token|secret)\s*[=:]\s*[^,; ]+")
        .map(|pattern| pattern.replace_all(&safe, "$1=***").into_owned())
        .unwrap_or(safe);
    regex::Regex::new(r"://[^/@ ]+@")
        .map(|pattern| pattern.replace_all(&redacted, "://***@").into_owned())
        .unwrap_or(redacted)
}

/// The error reply body `{"ok": false, "error": {"code", "message"}}`.
fn error_body(code: &str, message: impl Into<String>) -> Value {
    json!({ "ok": false, "error": { "code": code, "message": message.into() } })
}

fn describe_payload(commands: Vec<Value>, views: Vec<Value>, component: Option<Value>) -> Value {
    let provider = component
        .as_ref()
        .and_then(|component| component.get("component"))
        .and_then(Value::as_str)
        .unwrap_or("component")
        .to_string();
    let default_view = views
        .first()
        .and_then(|view| view.get("id"))
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let mut panels_map = serde_json::Map::new();
    panels_map.insert(
        "schemaVersion".to_string(),
        Value::String("edgecommons.panels.v2".to_string()),
    );
    panels_map.insert("provider".to_string(), Value::String(provider));
    panels_map.insert(
        "renderer".to_string(),
        Value::String("descriptor".to_string()),
    );
    if let Some(default_view) = default_view {
        panels_map.insert("defaultView".to_string(), Value::String(default_view));
    }
    panels_map.insert("views".to_string(), Value::Array(views));
    let panels = Value::Object(panels_map);
    let digest = descriptor_digest(&commands, &panels);
    let mut result = serde_json::Map::new();
    result.insert(
        "schemaVersion".to_string(),
        Value::String("edgecommons.component.describe.v1".to_string()),
    );
    if let Some(component) = component {
        result.insert("component".to_string(), component);
    }
    result.insert("digest".to_string(), Value::String(digest));
    result.insert("commands".to_string(), Value::Array(commands));
    result.insert("panels".to_string(), panels);
    Value::Object(result)
}

fn descriptor_digest(commands: &[Value], panels: &Value) -> String {
    let source = json!({ "commands": commands, "panels": panels });
    format!(
        "sha256:{}",
        sha256_hex(deterministic_json(&source).as_bytes())
    )
}

fn deterministic_json(value: &Value) -> String {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {
            serde_json::to_string(value).expect("serializing a scalar JSON value cannot fail")
        }
        Value::Array(values) => {
            let mut out = String::from("[");
            for (idx, item) in values.iter().enumerate() {
                if idx > 0 {
                    out.push(',');
                }
                out.push_str(&deterministic_json(item));
            }
            out.push(']');
            out
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut out = String::from("{");
            for (idx, key) in keys.iter().enumerate() {
                if idx > 0 {
                    out.push(',');
                }
                out.push_str(
                    &serde_json::to_string(key).expect("serializing a JSON object key cannot fail"),
                );
                out.push(':');
                out.push_str(&deterministic_json(&map[*key]));
            }
            out.push('}');
            out
        }
    }
}

fn sha256_hex(input: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let bit_len = (input.len() as u64) * 8;
    let mut bytes = input.to_vec();
    bytes.push(0x80);
    while bytes.len() % 64 != 56 {
        bytes.push(0);
    }
    bytes.extend_from_slice(&bit_len.to_be_bytes());

    let mut h = [
        0x6a09e667u32,
        0xbb67ae85,
        0x3c6ef372,
        0xa54ff53a,
        0x510e527f,
        0x9b05688c,
        0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut w = [0u32; 64];
    for chunk in bytes.chunks_exact(64) {
        for (idx, word) in w.iter_mut().take(16).enumerate() {
            let offset = idx * 4;
            *word = u32::from_be_bytes([
                chunk[offset],
                chunk[offset + 1],
                chunk[offset + 2],
                chunk[offset + 3],
            ]);
        }
        for idx in 16..64 {
            let s0 =
                w[idx - 15].rotate_right(7) ^ w[idx - 15].rotate_right(18) ^ (w[idx - 15] >> 3);
            let s1 = w[idx - 2].rotate_right(17) ^ w[idx - 2].rotate_right(19) ^ (w[idx - 2] >> 10);
            w[idx] = w[idx - 16]
                .wrapping_add(s0)
                .wrapping_add(w[idx - 7])
                .wrapping_add(s1);
        }

        let mut a = h[0];
        let mut b = h[1];
        let mut c = h[2];
        let mut d = h[3];
        let mut e = h[4];
        let mut f = h[5];
        let mut g = h[6];
        let mut hh = h[7];
        for idx in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[idx])
                .wrapping_add(w[idx]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    let mut out = String::with_capacity(64);
    for word in h {
        write!(&mut out, "{word:08x}").expect("writing to a String cannot fail");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::RecordingMessaging;
    use serde_json::json;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::time::Duration;

    // D-U28: the config identity is component scope, so the instance-scope filter renders the
    // instance slot as `+`; the inbox also subscribes the component-scope filter.
    const INBOX_FILTER: &str = "ecv1/test-thing/TestComponent/+/cmd/#";
    const COMPONENT_INBOX_FILTER: &str = "ecv1/test-thing/TestComponent/cmd/#";
    const REPLY_TO: &str = "edgecommons/reply-test-1";

    /// The two filters `start()` subscribes (instance-scope + component-scope, D-U28).
    fn inbox_filters() -> std::collections::HashSet<String> {
        std::collections::HashSet::from([
            INBOX_FILTER.to_string(),
            COMPONENT_INBOX_FILTER.to_string(),
        ])
    }

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
        MessageBuilder::new(verb, "1.0")
            .payload(json!({}))
            .reply_to(REPLY_TO)
            .build()
    }

    /// A well-formed fire-and-forget command (no `reply_to`).
    fn notification(verb: &str) -> Message {
        MessageBuilder::new(verb, "1.0").payload(json!({})).build()
    }

    /// A deterministic fixture: injected uptime/reload/redacted-config/instance-connectivity seams
    /// over a [`RecordingMessaging`], mirroring the Java `CommandInboxTest` fixture.
    struct Fixture {
        messaging: Arc<RecordingMessaging>,
        uptime: Arc<AtomicU64>,
        reload_ok: Arc<AtomicBool>,
        redacted: Arc<Mutex<Option<Value>>>,
        /// The one provider sample the `status` verb pulls (production: the heartbeat's).
        instances: Arc<Mutex<Vec<InstanceConnectivity>>>,
        inbox: Arc<CommandInbox>,
    }

    fn fixture() -> Fixture {
        let messaging = RecordingMessaging::new();
        let config = test_config();
        let uptime = Arc::new(AtomicU64::new(42));
        let reload_ok = Arc::new(AtomicBool::new(true));
        let redacted = Arc::new(Mutex::new(Some(
            json!({ "component": { "global": { "v": 1 } } }),
        )));
        let instances: Arc<Mutex<Vec<InstanceConnectivity>>> = Arc::new(Mutex::new(Vec::new()));

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
        let instance_connectivity: InstanceConnectivitySource = {
            let instances = instances.clone();
            Arc::new(move || instances.lock().unwrap().clone())
        };

        let inbox = CommandInbox::new(
            messaging.clone(),
            config,
            uptime_secs,
            reload_action,
            redacted_config,
            instance_connectivity,
        );
        Fixture {
            messaging,
            uptime,
            reload_ok,
            redacted,
            instances,
            inbox,
        }
    }

    /// The single recorded reply (topic must be the request's `reply_to`).
    fn only_reply_body(messaging: &RecordingMessaging) -> Value {
        let replies = messaging.replies();
        assert_eq!(replies.len(), 1, "exactly one reply expected");
        let (topic, msg) = &replies[0];
        assert_eq!(
            topic, REPLY_TO,
            "the reply must go to the request's reply_to"
        );
        msg.body.clone()
    }

    // ===================== subscription lifecycle =====================

    #[tokio::test]
    async fn start_subscribes_the_own_inbox_wildcard() {
        let f = fixture();
        f.inbox.clone().start().await;
        assert_eq!(
            f.messaging.subscribed_topics(),
            inbox_filters(),
            "start() must subscribe the instance- and component-scope cmd wildcards"
        );
    }

    #[tokio::test]
    async fn start_is_idempotent() {
        let f = fixture();
        f.inbox.clone().start().await;
        f.inbox.clone().start().await;
        assert_eq!(f.messaging.subscribed_topics(), inbox_filters());
    }

    #[tokio::test]
    async fn d_u28_dispatches_component_scope_and_instance_addressed_commands() {
        // A command lands whether addressed to an instance (`.../main/cmd/ping`, matches the
        // instance-scope `+` filter) or to the component (`.../cmd/ping`, matches the
        // component-scope filter); the verb is extracted via the `/cmd/` marker in both cases.
        let f = fixture();
        f.inbox.clone().start().await;

        f.messaging
            .simulate_message(&topic("ping"), request("ping"))
            .await;
        f.messaging
            .simulate_message("ecv1/test-thing/TestComponent/cmd/ping", request("ping"))
            .await;

        let replies = f.messaging.replies();
        assert_eq!(replies.len(), 2, "both scopes are dispatched and replied to");
        for (_, reply) in &replies {
            assert_eq!(reply.body["ok"], json!(true));
            assert_eq!(reply.body["result"]["status"], "RUNNING");
        }
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
        f.messaging
            .simulate_message(&topic(PING), request(PING))
            .await;
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
        f.messaging
            .simulate_message(&topic(PING), request(PING))
            .await;
        let body = only_reply_body(&f.messaging);
        assert!(body["ok"].as_bool().unwrap());
        assert_eq!(body["result"]["status"], "RUNNING");
        assert_eq!(body["result"]["uptimeSecs"], 1234);
    }

    /// A component with no instances (a plain service) answers `status` exactly as `ping` does —
    /// the `instances[]` section is omitted, never emitted empty.
    #[tokio::test]
    async fn status_without_instances_answers_exactly_as_ping() {
        let f = fixture();
        f.uptime.store(7, Ordering::SeqCst);
        f.inbox.clone().start().await;
        f.messaging
            .simulate_message(&topic(STATUS), request(STATUS))
            .await;
        let body = only_reply_body(&f.messaging);
        assert!(body["ok"].as_bool().unwrap());
        assert_eq!(
            body["result"],
            json!({ "status": "RUNNING", "uptimeSecs": 7 }),
            "no instances -> the section is omitted, so status == ping's body"
        );
    }

    /// `status` returns the SAME per-instance sample the `state` keepalive pushes (one provider,
    /// two surfaces), including the optional `state`/`attributes` members.
    #[tokio::test]
    async fn status_returns_the_instance_connectivity_sample() {
        let f = fixture();
        f.uptime.store(99, Ordering::SeqCst);
        *f.instances.lock().unwrap() = vec![
            InstanceConnectivity::new("kep1", true, Some("opc.tcp://kep:49320".to_string()))
                .with_state("ONLINE"),
            InstanceConnectivity::of("cam-2", false).with_attributes(
                json!({ "lastError": "timeout" })
                    .as_object()
                    .cloned()
                    .unwrap(),
            ),
        ];
        f.inbox.clone().start().await;
        f.messaging
            .simulate_message(&topic(STATUS), request(STATUS))
            .await;

        let body = only_reply_body(&f.messaging);
        assert!(body["ok"].as_bool().unwrap());
        let result = &body["result"];
        assert_eq!(result["status"], "RUNNING");
        assert_eq!(result["uptimeSecs"], 99);
        let instances = result["instances"].as_array().unwrap();
        assert_eq!(instances.len(), 2);
        assert_eq!(
            instances[0],
            json!({
                "instance": "kep1",
                "connected": true,
                "state": "ONLINE",
                "detail": "opc.tcp://kep:49320"
            })
        );
        assert_eq!(
            instances[1],
            json!({
                "instance": "cam-2",
                "connected": false,
                "attributes": { "lastError": "timeout" }
            })
        );
    }

    /// `status` is a built-in: it can be neither shadowed nor unregistered.
    #[tokio::test]
    async fn status_is_a_built_in_verb() {
        let f = fixture();
        assert!(BUILT_IN_VERBS.contains(&STATUS));
        assert!(matches!(
            f.inbox
                .register(STATUS, command_handler(|_r| async move { Ok(None) })),
            Err(EdgeCommonsError::Command(_))
        ));
        assert!(matches!(
            f.inbox.unregister(STATUS),
            Err(EdgeCommonsError::Command(_))
        ));
    }

    #[tokio::test]
    async fn reply_carries_the_request_correlation_id_verb_name_and_responder_identity() {
        let f = fixture();
        f.inbox.clone().start().await;
        let ping = request(PING);
        f.messaging
            .simulate_message(&topic(PING), ping.clone())
            .await;
        let replies = f.messaging.replies();
        let (_, reply) = &replies[0];
        assert_eq!(
            reply.header.correlation_id, ping.header.correlation_id,
            "the reply must carry the request's correlation_id"
        );
        assert_eq!(reply.header.name, PING, "the reply header.name is the verb");
        assert_eq!(reply.header.version, CMD_MESSAGE_VERSION);
        assert!(
            reply.identity.is_some(),
            "the reply is config-stamped with the responder's identity"
        );
    }

    #[tokio::test]
    async fn reload_config_replies_ack_on_success() {
        let f = fixture();
        f.inbox.clone().start().await;
        f.messaging
            .simulate_message(&topic(RELOAD_CONFIG), request(RELOAD_CONFIG))
            .await;
        let body = only_reply_body(&f.messaging);
        assert!(body["ok"].as_bool().unwrap());
        assert!(body["result"]["reloaded"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn reload_config_replies_reload_failed_on_failure() {
        let f = fixture();
        f.reload_ok.store(false, Ordering::SeqCst);
        f.inbox.clone().start().await;
        f.messaging
            .simulate_message(&topic(RELOAD_CONFIG), request(RELOAD_CONFIG))
            .await;
        let body = only_reply_body(&f.messaging);
        assert!(!body["ok"].as_bool().unwrap());
        assert_eq!(body["error"]["code"], ERR_RELOAD_FAILED);
        assert!(!body["error"]["message"].as_str().unwrap().is_empty());
    }

    #[tokio::test]
    async fn get_configuration_replies_the_redacted_effective_config() {
        let f = fixture();
        f.inbox.clone().start().await;
        f.messaging
            .simulate_message(&topic(GET_CONFIGURATION), request(GET_CONFIGURATION))
            .await;
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
        f.messaging
            .simulate_message(&topic(GET_CONFIGURATION), request(GET_CONFIGURATION))
            .await;
        let body = only_reply_body(&f.messaging);
        assert!(!body["ok"].as_bool().unwrap());
        assert_eq!(body["error"]["code"], ERR_NO_CONFIG);
    }

    #[tokio::test]
    async fn describe_includes_built_ins_custom_verbs_panels_and_digest() {
        let f = fixture();
        f.inbox
            .register("sb/browse", command_handler(|_r| async move { Ok(None) }))
            .unwrap();
        let panel = json!({
            "id": "address-space",
            "title": "Address Space",
            "order": 20,
            "widgets": [{ "kind": "treeBrowser", "browseVerb": "sb/browse" }]
        });
        f.inbox.register_panel(panel.clone()).unwrap();
        f.inbox.clone().start().await;

        f.messaging
            .simulate_message(&topic(DESCRIBE), request(DESCRIBE))
            .await;
        let body = only_reply_body(&f.messaging);
        assert!(body["ok"].as_bool().unwrap());
        let result = &body["result"];
        assert_eq!(result["schemaVersion"], "edgecommons.component.describe.v1");
        assert_eq!(result["component"]["path"], "test-thing");
        assert_eq!(result["component"]["hier"][0]["value"], "test-thing");
        assert_eq!(result["component"]["component"], "TestComponent");
        // D-U28: the fixture config is component scope, so describe omits the instance key.
        assert!(result["component"].get("instance").is_none());

        let commands = result["commands"].as_array().unwrap();
        for verb in BUILT_IN_VERBS {
            assert!(
                commands
                    .iter()
                    .any(|capability| capability["verb"] == verb && capability["builtIn"] == true),
                "describe must include built-in verb {verb}"
            );
        }
        assert!(
            commands
                .iter()
                .any(|capability| capability["verb"] == "sb/browse"
                    && capability["builtIn"] == false),
            "describe must include registered custom verbs"
        );
        assert_eq!(result["panels"]["schemaVersion"], "edgecommons.panels.v2");
        assert_eq!(result["panels"]["provider"], "TestComponent");
        assert_eq!(result["panels"]["renderer"], "descriptor");
        assert_eq!(result["panels"]["defaultView"], "address-space");
        assert_eq!(result["panels"]["views"], json!([panel]));

        let digest = result["digest"].as_str().unwrap();
        assert!(digest.starts_with("sha256:"));
        assert_eq!(digest.len(), "sha256:".len() + 64);
        assert!(
            digest["sha256:".len()..]
                .chars()
                .all(|c| c.is_ascii_hexdigit())
        );
        assert_eq!(
            digest,
            descriptor_digest(commands, &result["panels"]),
            "digest must be computed from deterministic commands+panels JSON"
        );
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
        f.messaging
            .simulate_message(&topic("restart-pipeline"), request("restart-pipeline"))
            .await;
        let body = only_reply_body(&f.messaging);
        assert!(body["ok"].as_bool().unwrap());
        assert!(body["result"]["restarted"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn namespaced_custom_verb_dispatches() {
        let f = fixture();
        f.inbox
            .register("sb/status", command_handler(|_req| async move { Ok(None) }))
            .unwrap();
        f.inbox.clone().start().await;
        f.messaging
            .simulate_message(&topic("sb/status"), request("sb/status"))
            .await;
        let body = only_reply_body(&f.messaging);
        assert!(body["ok"].as_bool().unwrap());
        assert_eq!(
            body["result"],
            json!({}),
            "a None handler result must reply an empty result object"
        );
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
        f.messaging
            .simulate_message(&topic("guarded"), request("guarded"))
            .await;
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
        f.messaging
            .simulate_message(&topic("boomy"), request("boomy"))
            .await;
        let body = only_reply_body(&f.messaging);
        assert!(!body["ok"].as_bool().unwrap());
        assert_eq!(body["error"]["code"], ERR_HANDLER_ERROR);
    }

    #[tokio::test]
    async fn register_rejects_shadowing_and_invalid_verbs() {
        let f = fixture();
        assert!(matches!(
            f.inbox
                .register(PING, command_handler(|_r| async move { Ok(None) })),
            Err(EdgeCommonsError::Command(_))
        ));
        assert!(matches!(
            f.inbox.register(
                SET_CONFIG_VERB,
                command_handler(|_r| async move { Ok(None) })
            ),
            Err(EdgeCommonsError::Command(_))
        ));
        f.inbox
            .register("mine", command_handler(|_r| async move { Ok(None) }))
            .unwrap();
        assert!(matches!(
            f.inbox
                .register("mine", command_handler(|_r| async move { Ok(None) })),
            Err(EdgeCommonsError::Command(_))
        ));
        assert!(matches!(
            f.inbox
                .register("bad+verb", command_handler(|_r| async move { Ok(None) })),
            Err(EdgeCommonsError::UnsValidation { .. })
        ));
        assert!(matches!(
            f.inbox
                .register("sb//x", command_handler(|_r| async move { Ok(None) })),
            Err(EdgeCommonsError::UnsValidation { .. })
        ));
    }

    #[test]
    fn register_panel_validates_required_fields_and_duplicate_ids() {
        let f = fixture();
        assert!(matches!(
            f.inbox.register_panel(json!("not-object")),
            Err(EdgeCommonsError::Command(_))
        ));
        assert!(matches!(
            f.inbox.register_panel(json!({ "title": "Missing id" })),
            Err(EdgeCommonsError::Command(_))
        ));
        assert!(matches!(
            f.inbox
                .register_panel(json!({ "id": "", "title": "Empty id" })),
            Err(EdgeCommonsError::Command(_))
        ));
        assert!(matches!(
            f.inbox.register_panel(json!({ "id": "missing-title" })),
            Err(EdgeCommonsError::Command(_))
        ));
        assert!(matches!(
            f.inbox
                .register_panel(json!({ "id": "empty-title", "title": "" })),
            Err(EdgeCommonsError::Command(_))
        ));

        let panel = json!({ "id": "overview", "title": "Overview" });
        f.inbox.register_panel(panel.clone()).unwrap();
        assert_eq!(f.inbox.panels(), vec![panel]);
        assert!(matches!(
            f.inbox
                .register_panel(json!({ "id": "overview", "title": "Duplicate" })),
            Err(EdgeCommonsError::Command(_))
        ));
    }

    #[tokio::test]
    async fn unregister_removes_custom_verbs_but_never_built_ins() {
        let f = fixture();
        f.inbox
            .register("mine", command_handler(|_r| async move { Ok(None) }))
            .unwrap();
        assert!(f.inbox.verbs().contains("mine"));
        f.inbox.unregister("mine").unwrap();
        assert!(!f.inbox.verbs().contains("mine"));
        f.inbox.unregister("mine").unwrap(); // unknown -> no-op
        assert!(matches!(
            f.inbox.unregister(RELOAD_CONFIG),
            Err(EdgeCommonsError::Command(_))
        ));

        // The unregistered verb now gets the unknown-verb error.
        f.inbox.clone().start().await;
        f.messaging
            .simulate_message(&topic("mine"), request("mine"))
            .await;
        assert_eq!(
            only_reply_body(&f.messaging)["error"]["code"],
            ERR_UNKNOWN_VERB
        );
    }

    #[tokio::test]
    async fn verbs_snapshot_contains_built_ins_and_customs() {
        let f = fixture();
        f.inbox
            .register("mine", command_handler(|_r| async move { Ok(None) }))
            .unwrap();
        assert_eq!(
            f.inbox.verbs(),
            std::collections::HashSet::from([
                PING.to_string(),
                DESCRIBE.to_string(),
                RELOAD_CONFIG.to_string(),
                GET_CONFIGURATION.to_string(),
                STATUS.to_string(),
                "mine".to_string(),
            ])
        );
    }

    // ===================== explicit outcomes + deferred replies =====================

    #[tokio::test]
    async fn outcome_handler_immediate_result_uses_the_standard_wrapper() {
        let f = fixture();
        f.inbox
            .register_outcome(
                "sb/status",
                outcome_handler(|_request, _deferred| async move {
                    CommandOutcome::ImmediateSuccess(Some(json!({ "online": true })))
                }),
            )
            .unwrap();
        assert!(
            f.inbox
                .register(
                    "sb/status",
                    command_handler(|_request| async move { Ok(None) })
                )
                .is_err(),
            "legacy and outcome handlers share one no-shadowing verb namespace"
        );
        f.inbox.clone().start().await;

        f.messaging
            .simulate_message(&topic("sb/status"), request("sb/status"))
            .await;

        let body = only_reply_body(&f.messaging);
        assert_eq!(body, json!({ "ok": true, "result": { "online": true } }));
    }

    #[test]
    fn defer_rejects_a_request_without_reply_to() {
        let f = fixture();
        let error = f
            .inbox
            .defer(&notification("sb/capture"), Duration::from_secs(1))
            .unwrap_err();
        assert_eq!(error.code, ERR_REPLY_REQUIRED);
    }

    #[test]
    fn defer_rejects_missing_correlation_or_verb() {
        let f = fixture();
        let mut missing_correlation = request("sb/capture");
        missing_correlation.header.correlation_id.clear();
        let error = f
            .inbox
            .defer(&missing_correlation, Duration::from_secs(1))
            .unwrap_err();
        assert_eq!(error.code, ERR_INVALID_DEFERRED_TOKEN);

        let mut missing_verb = request("sb/capture");
        missing_verb.header.name.clear();
        let error = f
            .inbox
            .defer(&missing_verb, Duration::from_secs(1))
            .unwrap_err();
        assert_eq!(error.code, ERR_INVALID_DEFERRED_TOKEN);
    }

    #[tokio::test]
    async fn deferred_outcome_suppresses_automatic_reply_and_settles_later() {
        let f = fixture();
        let token_slot = Arc::new(Mutex::new(None::<DeferredReplyToken>));
        let token_slot_handler = token_slot.clone();
        f.inbox
            .register_outcome(
                "sb/capture",
                outcome_handler(move |request, deferred| {
                    let token_slot = token_slot_handler.clone();
                    async move {
                        let token = match deferred.defer(&request, Duration::from_secs(2)) {
                            Ok(token) => token,
                            Err(error) => return CommandOutcome::ImmediateError(error),
                        };
                        // Represents the application job insert/commit.
                        token.activate().unwrap();
                        *token_slot.lock().unwrap() = Some(token.clone());
                        CommandOutcome::Deferred(token)
                    }
                }),
            )
            .unwrap();
        f.inbox.clone().start().await;
        let capture = request("sb/capture");

        f.messaging
            .simulate_message(&topic("sb/capture"), capture.clone())
            .await;
        assert!(
            f.messaging.replies().is_empty(),
            "returning Deferred must release dispatch without an automatic reply"
        );

        let token = token_slot.lock().unwrap().take().unwrap();
        token
            .settle_success(Some(json!({ "captureId": "cap-1" })))
            .await
            .unwrap();
        let replies = f.messaging.replies();
        assert_eq!(replies.len(), 1);
        assert_eq!(
            replies[0].1.header.correlation_id,
            capture.header.correlation_id
        );
        assert_eq!(replies[0].1.body["result"]["captureId"], "cap-1");
    }

    #[tokio::test]
    async fn post_accept_continuation_runs_only_after_the_open_token_is_accepted() {
        let f = fixture();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let started_tx = Arc::new(Mutex::new(Some(started_tx)));
        let started_tx_handler = started_tx.clone();
        f.inbox
            .register_outcome(
                "sb/capture",
                outcome_handler(move |request, deferred| {
                    let started_tx = started_tx_handler.clone();
                    async move {
                        let token = match deferred.defer(&request, Duration::from_secs(2)) {
                            Ok(token) => token,
                            Err(error) => return CommandOutcome::ImmediateError(error),
                        };
                        if let Err(error) = token.activate() {
                            return CommandOutcome::ImmediateError(CommandError::handler_error(
                                error.to_string(),
                            ));
                        }
                        let settlement = token.clone();
                        CommandOutcome::deferred_with_continuation(token, async move {
                            if let Some(sender) =
                                started_tx.lock().ok().and_then(|mut slot| slot.take())
                            {
                                let _ = sender.send(());
                            }
                            let _ = settlement
                                .settle_success(Some(json!({ "captureId": "cap-post-accept" })))
                                .await;
                            Ok(())
                        })
                    }
                }),
            )
            .unwrap();
        f.inbox.clone().start().await;

        f.messaging
            .simulate_message(&topic("sb/capture"), request("sb/capture"))
            .await;

        tokio::time::timeout(Duration::from_secs(1), started_rx)
            .await
            .expect("the inbox-owned continuation should start")
            .expect("the continuation should report its start");
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if !f.messaging.replies().is_empty() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the continuation should settle through the guarded token");
        assert_eq!(
            only_reply_body(&f.messaging),
            json!({ "ok": true, "result": { "captureId": "cap-post-accept" } })
        );
    }

    #[tokio::test]
    async fn invalid_post_accept_token_never_starts_its_continuation() {
        let f = fixture();
        let ran = Arc::new(AtomicBool::new(false));
        let ran_handler = ran.clone();
        f.inbox
            .register_outcome(
                "sb/capture",
                outcome_handler(move |request, deferred| {
                    let ran = ran_handler.clone();
                    async move {
                        let token = match deferred.defer(&request, Duration::from_secs(2)) {
                            Ok(token) => token,
                            Err(error) => return CommandOutcome::ImmediateError(error),
                        };
                        // Intentionally leave the token PROVISIONAL. The inbox must reject it
                        // before the continuation can run.
                        CommandOutcome::deferred_with_continuation(token, async move {
                            ran.store(true, Ordering::SeqCst);
                            Ok(())
                        })
                    }
                }),
            )
            .unwrap();
        f.inbox.clone().start().await;

        f.messaging
            .simulate_message(&topic("sb/capture"), request("sb/capture"))
            .await;
        tokio::task::yield_now().await;

        assert!(!ran.load(Ordering::SeqCst));
        assert_eq!(
            only_reply_body(&f.messaging)["error"]["code"],
            ERR_INVALID_DEFERRED_TOKEN
        );
    }

    #[tokio::test]
    async fn failed_post_accept_continuation_settles_through_the_guarded_error_path() {
        let f = fixture();
        f.inbox
            .register_outcome(
                "sb/capture",
                outcome_handler(|request, deferred| async move {
                    let token = match deferred.defer(&request, Duration::from_secs(2)) {
                        Ok(token) => token,
                        Err(error) => return CommandOutcome::ImmediateError(error),
                    };
                    if let Err(error) = token.activate() {
                        return CommandOutcome::ImmediateError(CommandError::handler_error(
                            error.to_string(),
                        ));
                    }
                    CommandOutcome::deferred_with_continuation(token, async move {
                        Err(CommandError::new(
                            "CAMERA_FAILED",
                            "simulated camera worker failure",
                        ))
                    })
                }),
            )
            .unwrap();
        f.inbox.clone().start().await;

        f.messaging
            .simulate_message(&topic("sb/capture"), request("sb/capture"))
            .await;
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if !f.messaging.replies().is_empty() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("failed continuation should settle through the guarded token");
        assert_eq!(
            only_reply_body(&f.messaging)["error"]["code"],
            "CAMERA_FAILED"
        );
    }

    #[tokio::test]
    async fn provisional_token_returned_as_deferred_is_rejected_and_discarded() {
        let f = fixture();
        f.inbox
            .register_outcome(
                "sb/capture",
                outcome_handler(|request, deferred| async move {
                    let token = deferred.defer(&request, Duration::from_secs(1)).unwrap();
                    CommandOutcome::Deferred(token)
                }),
            )
            .unwrap();
        f.inbox.clone().start().await;

        f.messaging
            .simulate_message(&topic("sb/capture"), request("sb/capture"))
            .await;

        assert_eq!(
            only_reply_body(&f.messaging)["error"]["code"],
            ERR_INVALID_DEFERRED_TOKEN
        );
    }

    #[tokio::test]
    async fn invalid_outcome_does_not_discard_an_unrelated_provisional_token() {
        let f = fixture();
        let unrelated_request = request("sb/other");
        let unrelated = f
            .inbox
            .defer(&unrelated_request, Duration::from_secs(1))
            .unwrap();
        let returned = unrelated.clone();
        f.inbox
            .register_outcome(
                "sb/capture",
                outcome_handler(move |_request, _deferred| {
                    let returned = returned.clone();
                    async move { CommandOutcome::Deferred(returned) }
                }),
            )
            .unwrap();
        f.inbox.clone().start().await;

        f.messaging
            .simulate_message(&topic("sb/capture"), request("sb/capture"))
            .await;

        assert_eq!(
            only_reply_body(&f.messaging)["error"]["code"],
            ERR_INVALID_DEFERRED_TOKEN
        );
        unrelated
            .discard()
            .expect("the unrelated provisional token must remain untouched");
    }

    #[tokio::test]
    async fn cloned_deferred_tokens_can_settle_at_most_once() {
        let f = fixture();
        let request = request("sb/capture");
        let token = f.inbox.defer(&request, Duration::from_secs(1)).unwrap();
        token.activate().unwrap();
        let other = token.clone();

        let (first, second) = tokio::join!(
            token.settle_success(Some(json!({ "winner": 1 }))),
            other.settle_success(Some(json!({ "winner": 2 })))
        );

        assert_ne!(first.is_ok(), second.is_ok(), "exactly one CAS winner");
        let loser = first.err().or_else(|| second.err()).unwrap();
        assert_eq!(
            loser.to_string(),
            "command error: deferred reply is already being settled or has settled"
        );
        assert_eq!(f.messaging.replies().len(), 1, "exactly one reply");
    }

    #[tokio::test]
    async fn deferred_settlement_retries_confirmed_reply_until_success() {
        let f = fixture();
        f.messaging.fail_next_confirmed(2);
        let request = request("sb/capture");
        let token = f.inbox.defer(&request, Duration::from_secs(2)).unwrap();
        token.activate().unwrap();

        token
            .settle_success(Some(json!({ "captureId": "cap-retry" })))
            .await
            .unwrap();

        assert_eq!(f.messaging.replies().len(), 1);
        assert_eq!(
            f.messaging.replies()[0].1.body["result"]["captureId"],
            "cap-retry"
        );
    }

    #[tokio::test]
    async fn open_deferred_token_expires_by_timer() {
        let f = fixture();
        let request = request("sb/capture");
        let token = f.inbox.defer(&request, Duration::from_millis(20)).unwrap();
        token.activate().unwrap();

        tokio::time::sleep(Duration::from_millis(50)).await;

        assert!(token.settle_success(None).await.is_err());
        assert!(f.messaging.replies().is_empty());
    }

    #[tokio::test]
    async fn deferred_registry_is_bounded_to_1024_entries() {
        let f = fixture();
        let request = request("sb/capture");
        let mut tokens = Vec::with_capacity(DEFERRED_REPLY_CAPACITY);
        for _ in 0..DEFERRED_REPLY_CAPACITY {
            tokens.push(f.inbox.defer(&request, Duration::from_secs(60)).unwrap());
        }

        let error = f
            .inbox
            .defer(&request, Duration::from_secs(60))
            .unwrap_err();
        assert_eq!(error.code, ERR_DEFERRED_CAPACITY);
        for token in tokens {
            token.discard().unwrap();
        }
    }

    #[tokio::test]
    async fn shutdown_attempts_component_stopping_reply_then_cancels_open_token() {
        let f = fixture();
        let request = request("sb/capture");
        let token = f.inbox.defer(&request, Duration::from_secs(2)).unwrap();
        token.activate().unwrap();

        f.inbox.shutdown_deferred().await;

        let body = only_reply_body(&f.messaging);
        assert_eq!(body["error"]["code"], ERR_COMPONENT_STOPPING);
        assert!(token.settle_success(None).await.is_err());
        assert_eq!(
            f.inbox
                .defer(&request, Duration::from_secs(1))
                .unwrap_err()
                .code,
            ERR_COMPONENT_STOPPING
        );
    }

    // ===================== unknown / fire-and-forget / malformed =====================

    #[tokio::test]
    async fn unknown_verb_request_gets_an_unknown_verb_error_reply() {
        let f = fixture();
        f.inbox.clone().start().await;
        f.messaging
            .simulate_message(&topic("no-such-verb"), request("no-such-verb"))
            .await;
        let body = only_reply_body(&f.messaging);
        assert!(!body["ok"].as_bool().unwrap());
        assert_eq!(body["error"]["code"], ERR_UNKNOWN_VERB);
    }

    #[tokio::test]
    async fn unknown_fire_and_forget_verb_is_ignored() {
        let f = fixture();
        f.inbox.clone().start().await;
        f.messaging
            .simulate_message(&topic("no-such-verb"), notification("no-such-verb"))
            .await;
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
        f.messaging
            .simulate_message(&topic("do-it"), notification("do-it"))
            .await;
        assert!(
            ran.load(Ordering::SeqCst),
            "a fire-and-forget command must still run the handler"
        );
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
        f.messaging
            .simulate_message(&topic("do-it"), notification("do-it"))
            .await;
        assert!(f.messaging.replies().is_empty());
    }

    #[tokio::test]
    async fn malformed_payloads_are_ignored_without_reply_and_never_crash() {
        let f = fixture();
        f.inbox.clone().start().await;
        // header.name does not equal the topic verb (foreign convention on a cmd topic).
        f.messaging
            .simulate_message(&topic(PING), request("something-else"))
            .await;
        // A raw (headerless) envelope - junk JSON on the inbox.
        f.messaging
            .simulate_message(&topic(PING), Message::raw(json!({ "junk": true })))
            .await;
        assert!(
            f.messaging.replies().is_empty(),
            "malformed/foreign payloads must never be replied to"
        );
    }

    #[tokio::test]
    async fn delegated_set_config_is_ignored_even_as_a_request() {
        let f = fixture();
        f.inbox.clone().start().await;
        f.messaging
            .simulate_message(&topic(SET_CONFIG_VERB), request(SET_CONFIG_VERB))
            .await;
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
        f.messaging
            .simulate_message(&topic(PING), request(PING))
            .await;
        assert!(f.messaging.replies().is_empty());
    }

    #[tokio::test]
    async fn acknowledged_start_gates_pre_active_deliveries_in_order() {
        let f = fixture();
        let observed = Arc::new(Mutex::new(Vec::new()));
        let observed_handler = observed.clone();
        f.inbox
            .register(
                "capture",
                command_handler(move |request| {
                    let observed = observed_handler.clone();
                    async move {
                        observed
                            .lock()
                            .unwrap()
                            .push(request.body["seq"].as_u64().unwrap());
                        Ok(None)
                    }
                }),
            )
            .unwrap();
        f.messaging
            .block_subscribe_ack
            .store(true, Ordering::SeqCst);
        *f.messaging.deliver_during_subscribe_ack.lock().unwrap() = (0..6)
            .map(|seq| {
                (
                    topic("capture"),
                    MessageBuilder::new("capture", "1.0")
                        .payload(json!({ "seq": seq }))
                        .build(),
                )
            })
            .collect();

        let inbox = f.inbox.clone();
        let start = tokio::spawn(async move { inbox.start().await });
        tokio::time::timeout(
            Duration::from_secs(1),
            f.messaging.subscribe_ack_entered.notified(),
        )
        .await
        .unwrap();
        assert_eq!(
            f.inbox.startup_status().state,
            CommandInboxStartupState::Starting
        );
        assert!(observed.lock().unwrap().is_empty());

        f.messaging.subscribe_ack_release.notify_one();
        assert_eq!(start.await.unwrap().state, CommandInboxStartupState::Active);
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if observed.lock().unwrap().len() == 6 {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(*observed.lock().unwrap(), vec![0, 1, 2, 3, 4, 5]);
    }

    #[tokio::test]
    async fn activation_gate_is_strictly_bounded_and_drops_newest() {
        let f = fixture();
        let observed = Arc::new(Mutex::new(Vec::new()));
        let observed_handler = observed.clone();
        f.inbox
            .register(
                "capture",
                command_handler(move |request| {
                    let observed = observed_handler.clone();
                    async move {
                        observed
                            .lock()
                            .unwrap()
                            .push(request.body["seq"].as_u64().unwrap());
                        Ok(None)
                    }
                }),
            )
            .unwrap();
        f.messaging
            .block_subscribe_ack
            .store(true, Ordering::SeqCst);
        *f.messaging.deliver_during_subscribe_ack.lock().unwrap() = (0
            ..=MAX_PENDING_STARTUP_DELIVERIES)
            .map(|seq| {
                (
                    topic("capture"),
                    MessageBuilder::new("capture", "1.0")
                        .payload(json!({ "seq": seq }))
                        .build(),
                )
            })
            .collect();

        let inbox = f.inbox.clone();
        let start = tokio::spawn(async move { inbox.start().await });
        f.messaging.subscribe_ack_entered.notified().await;
        f.messaging.subscribe_ack_release.notify_one();
        assert_eq!(start.await.unwrap().state, CommandInboxStartupState::Active);
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if observed.lock().unwrap().len() == MAX_PENDING_STARTUP_DELIVERIES {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            *observed.lock().unwrap(),
            (0..MAX_PENDING_STARTUP_DELIVERIES as u64).collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn stop_during_ack_invalidates_generation_and_restart_is_clean() {
        let f = fixture();
        f.messaging
            .block_subscribe_ack
            .store(true, Ordering::SeqCst);
        let inbox = f.inbox.clone();
        let start = tokio::spawn(async move { inbox.start().await });
        f.messaging.subscribe_ack_entered.notified().await;

        assert_eq!(
            f.inbox.stop().await.state,
            CommandInboxStartupState::Stopped
        );
        f.messaging.subscribe_ack_release.notify_one();
        assert_eq!(
            start.await.unwrap().state,
            CommandInboxStartupState::Stopped
        );
        assert!(f.messaging.subscribed_topics().is_empty());

        f.messaging
            .block_subscribe_ack
            .store(false, Ordering::SeqCst);
        assert_eq!(
            f.inbox.clone().start().await.state,
            CommandInboxStartupState::Active
        );
        assert_eq!(f.messaging.subscribed_topics(), inbox_filters());
    }

    #[tokio::test]
    async fn ack_failure_is_sanitized_and_retryable() {
        let f = fixture();
        *f.messaging.subscribe_ack_failure.lock().unwrap() =
            Some("token=secret mqtt://user:password@broker\nfailed".to_string());
        let failed = f.inbox.clone().start().await;
        assert_eq!(failed.state, CommandInboxStartupState::Failed);
        assert!(!failed.error.contains("secret"));
        assert!(!failed.error.contains("user:password"));
        assert!(!failed.error.contains('\n'));
        assert!(f.messaging.subscribed_topics().is_empty());

        *f.messaging.subscribe_ack_failure.lock().unwrap() = None;
        assert_eq!(
            f.inbox.clone().start().await.state,
            CommandInboxStartupState::Active
        );
    }
}

/// Cross-language conformance against `uns-test-vectors/commands.json` (DESIGN-uns §9.5, the
/// edge-console slice S2): the inbox filter byte-for-byte, the built-in verb goldens replayed
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
        let bytes =
            std::fs::read(dir.join(file)).unwrap_or_else(|e| panic!("failed to read {file}: {e}"));
        serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("{file} is not valid JSON: {e}"))
    }

    fn str_field<'a>(v: &'a Value, key: &str) -> &'a str {
        v.get(key)
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("missing string '{key}' in {v}"))
    }

    /// The `gw-01`/`opcua-adapter`/`main` identity every case in `commands.json` is keyed to.
    /// The vectors pin an instance-scoped component (`main`); config resolution is component
    /// scope by D-U28, so the identity is rebound to `main` (mirroring the Java loader's
    /// `MockConfigurationService.setComponentIdentity`).
    fn vector_config() -> Arc<ArcSwap<Config>> {
        Arc::new(ArcSwap::from_pointee(
            Config::from_value("opcua-adapter", "gw-01", json!({}))
                .unwrap()
                .with_instance_for_test("main"),
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
        assert!(
            !input["includeRoot"].as_bool().unwrap(),
            "the vectors are rootless"
        );
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
        // The golden `status` reply carries no instances[]: the vectors pin a component with no
        // registered connectivity provider, so status answers exactly as ping.
        let instance_connectivity: InstanceConnectivitySource = Arc::new(Vec::new);
        let inbox = CommandInbox::new(
            messaging.clone(),
            config,
            Arc::new(|| 42u64),
            reload_action,
            redacted_config,
            instance_connectivity,
        );
        inbox.clone().start().await;
        assert_eq!(
            messaging.subscribed_topics(),
            std::collections::HashSet::from([
                str_field(&doc["inbox"], "filter").to_string(),
                str_field(&doc["inbox"], "componentFilter").to_string(),
            ]),
            "start() must subscribe the pinned instance- and component-scope filters (D-U28)"
        );

        // ---- the built-in verb goldens, replayed through the live inbox ----
        let verbs = doc["verbs"].as_array().expect("verbs group");
        assert_eq!(
            verbs.len(),
            BUILT_IN_VERBS.len(),
            "built-in command goldens"
        );
        for case in verbs {
            let verb = str_field(case, "verb");
            let topic = str_field(case, "topic");
            let request = request_message(&case["request"]);
            let expected_reply_to = str_field(&case["request"]["header"], "reply_to");
            let expected_correlation_id = str_field(&case["request"]["header"], "correlation_id");

            messaging.simulate_message(topic, request).await;

            let replies = messaging.replies();
            let (reply_topic, reply) = replies
                .last()
                .unwrap_or_else(|| panic!("verb '{verb}': no reply recorded"));
            assert_eq!(
                reply_topic, expected_reply_to,
                "verb '{verb}': reply topic mismatch"
            );
            assert_eq!(
                reply.header.name, verb,
                "verb '{verb}': reply header.name mismatch"
            );
            assert_eq!(
                reply.header.version, "1.0",
                "verb '{verb}': reply header.version mismatch"
            );
            assert_eq!(
                reply.header.correlation_id, expected_correlation_id,
                "verb '{verb}': reply must carry the request's correlation_id"
            );
            assert_eq!(
                reply.body, case["reply"]["body"],
                "verb '{verb}': reply body mismatch"
            );

            let identity = reply
                .identity
                .as_ref()
                .unwrap_or_else(|| panic!("verb '{verb}': reply carries no identity"));
            let expected_identity = &case["reply"]["identity"];
            assert_eq!(identity.path(), str_field(expected_identity, "path"));
            assert_eq!(
                identity.component(),
                str_field(expected_identity, "component")
            );
            // D-U28: a config-stamped reply is component scope (no instance), regardless of the
            // vector's instance-scoped golden identity. The Java loader likewise asserts only the
            // live body/topic/correlation, not the live reply's instance.
            assert_eq!(
                identity.instance(),
                None,
                "verb '{verb}': the live reply identity is component scope"
            );
        }

        // ---- UNKNOWN_VERB (the library-composed message text is pinned) ----
        let errors = doc["errors"].as_array().expect("errors group");
        assert_eq!(errors.len(), 1, "unknown-verb");
        let unknown = &errors[0];
        let request = request_message(&unknown["request"]);
        messaging
            .simulate_message(str_field(unknown, "topic"), request)
            .await;
        let (_, reply) = messaging.replies().last().unwrap().clone();
        assert_eq!(
            reply.body, unknown["reply"]["body"],
            "UNKNOWN_VERB reply body mismatch"
        );

        // ---- behavior flags/sets (normative for every language's command inbox) ----
        let behavior = &doc["behavior"];
        assert!(behavior["verbIsTopicChannel"].as_bool().unwrap());
        assert!(behavior["headerNameMustEqualVerb"].as_bool().unwrap());
        assert!(behavior["fireAndForgetWithoutReplyTo"].as_bool().unwrap());
        assert!(behavior["malformedIgnoredWithoutReply"].as_bool().unwrap());
        let built_ins: Vec<&str> = behavior["builtInVerbs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(built_ins, BUILT_IN_VERBS.to_vec(), "builtInVerbs");
        let delegated: Vec<&str> = behavior["delegatedVerbs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(delegated, DELEGATED_VERBS.to_vec(), "delegatedVerbs");
        let error_codes: std::collections::HashSet<&str> = behavior["errorCodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
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
