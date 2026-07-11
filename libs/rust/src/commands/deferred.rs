//! Inbox-owned deferred command-reply registry.

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use arc_swap::ArcSwap;
use serde_json::{Value, json};
use tokio::time::Instant;
use uuid::Uuid;

use super::{
    CMD_MESSAGE_VERSION, CommandError, ERR_COMPONENT_STOPPING, ERR_DEFERRED_CAPACITY,
    ERR_INVALID_DEFERRED_TOKEN, ERR_REPLY_REQUIRED, error_body,
};
use crate::config::model::Config;
use crate::error::{EdgeCommonsError, Result};
use crate::messaging::MessagingService;
use crate::messaging::message::{Message, MessageBuilder};

/// Maximum provisional/open/settling/settled entries owned by one command inbox.
pub const DEFERRED_REPLY_CAPACITY: usize = 1024;

const PROVISIONAL: u8 = 0;
const OPEN: u8 = 1;
const SETTLING: u8 = 2;
const SETTLED: u8 = 3;
const DISCARDED: u8 = 4;
const EXPIRED: u8 = 5;
const CANCELLED_ON_SHUTDOWN: u8 = 6;

const CONFIRM_ATTEMPT_MAX: Duration = Duration::from_secs(5);
const RETRY_MIN: Duration = Duration::from_millis(100);
const RETRY_MAX: Duration = Duration::from_secs(1);
const SHUTDOWN_CONFIRM_MAX: Duration = Duration::from_secs(1);

fn state_name(state: u8) -> &'static str {
    match state {
        PROVISIONAL => "PROVISIONAL",
        OPEN => "OPEN",
        SETTLING => "SETTLING",
        SETTLED => "SETTLED",
        DISCARDED => "DISCARDED",
        EXPIRED => "EXPIRED",
        CANCELLED_ON_SHUTDOWN => "CANCELLED_ON_SHUTDOWN",
        _ => "UNKNOWN",
    }
}

struct DeferredEntry {
    id: Uuid,
    request: Message,
    verb: String,
    expires_at: Instant,
    state: AtomicU8,
}

struct RegistryInner {
    messaging: Arc<dyn MessagingService>,
    config: Arc<ArcSwap<Config>>,
    entries: Mutex<HashMap<Uuid, Arc<DeferredEntry>>>,
    shutting_down: AtomicBool,
}

/// Cloneable handle to the command inbox's bounded deferred-reply registry.
///
/// The registry retains guarded request metadata; callers receive only [`DeferredReplyToken`].
#[derive(Clone)]
pub struct DeferredReplyRegistry {
    inner: Arc<RegistryInner>,
}

impl fmt::Debug for DeferredReplyRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeferredReplyRegistry")
            .field("capacity", &DEFERRED_REPLY_CAPACITY)
            .finish_non_exhaustive()
    }
}

/// Opaque capability for one retained deferred request.
///
/// Cloning a token does not permit duplicate replies: settlement is guarded by an atomic
/// `OPEN -> SETTLING` compare-and-set. The token exposes no `reply_to` or raw publish primitive.
#[derive(Clone)]
pub struct DeferredReplyToken {
    id: Uuid,
    registry: Weak<RegistryInner>,
}

impl fmt::Debug for DeferredReplyToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DeferredReplyToken(..)")
    }
}

impl DeferredReplyRegistry {
    pub(super) fn new(messaging: Arc<dyn MessagingService>, config: Arc<ArcSwap<Config>>) -> Self {
        Self {
            inner: Arc::new(RegistryInner {
                messaging,
                config,
                entries: Mutex::new(HashMap::new()),
                shutting_down: AtomicBool::new(false),
            }),
        }
    }

    /// Create a provisional token for `request` with an explicit positive lifetime.
    ///
    /// The caller must durably accept its work and call [`DeferredReplyToken::activate`] before
    /// returning the token as a deferred command outcome. On acceptance failure it calls
    /// [`DeferredReplyToken::discard`] and returns an immediate command error.
    pub fn defer(
        &self,
        request: &Message,
        lifetime: Duration,
    ) -> std::result::Result<DeferredReplyToken, CommandError> {
        if request.header.reply_to.as_deref().is_none_or(str::is_empty) {
            return Err(CommandError::new(
                ERR_REPLY_REQUIRED,
                "a deferred command requires a non-empty reply_to",
            ));
        }
        if request.header.correlation_id.is_empty() {
            return Err(CommandError::new(
                ERR_INVALID_DEFERRED_TOKEN,
                "a deferred command requires a non-empty correlation id",
            ));
        }
        if request.header.name.is_empty() {
            return Err(CommandError::new(
                ERR_INVALID_DEFERRED_TOKEN,
                "a deferred command requires a non-empty verb",
            ));
        }
        if lifetime.is_zero() {
            return Err(CommandError::new(
                ERR_INVALID_DEFERRED_TOKEN,
                "a deferred command requires a positive lifetime",
            ));
        }
        if self.inner.shutting_down.load(Ordering::Acquire) {
            return Err(CommandError::new(
                ERR_COMPONENT_STOPPING,
                "the command inbox is shutting down",
            ));
        }
        let handle = tokio::runtime::Handle::try_current().map_err(|_| {
            CommandError::new(
                ERR_INVALID_DEFERRED_TOKEN,
                "deferred replies require an active Tokio runtime",
            )
        })?;
        let now = Instant::now();
        let expires_at = now.checked_add(lifetime).ok_or_else(|| {
            CommandError::new(
                ERR_INVALID_DEFERRED_TOKEN,
                "deferred reply lifetime is too large",
            )
        })?;
        let id = Uuid::new_v4();
        let entry = Arc::new(DeferredEntry {
            id,
            request: request.clone(),
            verb: request.header.name.clone(),
            expires_at,
            state: AtomicU8::new(PROVISIONAL),
        });
        {
            let mut entries = self.inner.entries.lock().map_err(|_| {
                CommandError::new(
                    ERR_INVALID_DEFERRED_TOKEN,
                    "the deferred reply registry is unavailable",
                )
            })?;
            // Serialize the final shutdown check with insertion. If defer holds the entries lock
            // first, shutdown's subsequent snapshot includes this token; if shutdown wins first,
            // insertion is rejected. No token can slip behind the shutdown snapshot.
            if self.inner.shutting_down.load(Ordering::Acquire) {
                return Err(CommandError::new(
                    ERR_COMPONENT_STOPPING,
                    "the command inbox is shutting down",
                ));
            }
            if entries.len() >= DEFERRED_REPLY_CAPACITY {
                return Err(CommandError::new(
                    ERR_DEFERRED_CAPACITY,
                    format!(
                        "the deferred reply registry is full (capacity {DEFERRED_REPLY_CAPACITY})"
                    ),
                ));
            }
            entries.insert(id, entry);
        }

        let weak = Arc::downgrade(&self.inner);
        handle.spawn(async move {
            tokio::time::sleep_until(expires_at).await;
            if let Some(inner) = weak.upgrade() {
                inner.expire(id);
            }
        });

        Ok(DeferredReplyToken {
            id,
            registry: Arc::downgrade(&self.inner),
        })
    }

    pub(super) fn validate_open_token(
        &self,
        token: &DeferredReplyToken,
        request: &Message,
    ) -> Result<()> {
        let Some(owner) = token.registry.upgrade() else {
            return Err(EdgeCommonsError::Command(
                "deferred token owner no longer exists".to_string(),
            ));
        };
        if !Arc::ptr_eq(&owner, &self.inner) {
            return Err(EdgeCommonsError::Command(
                "deferred token belongs to another command inbox".to_string(),
            ));
        }
        let entry = owner.entry(token.id)?;
        if entry.state.load(Ordering::Acquire) != OPEN {
            return Err(EdgeCommonsError::Command(format!(
                "deferred token is {}, not OPEN",
                state_name(entry.state.load(Ordering::Acquire))
            )));
        }
        if entry.request.header.uuid != request.header.uuid
            || entry.request.header.name != request.header.name
            || entry.request.header.correlation_id != request.header.correlation_id
            || entry.request.header.reply_to != request.header.reply_to
        {
            return Err(EdgeCommonsError::Command(
                "deferred token does not belong to this exact request".to_string(),
            ));
        }
        Ok(())
    }

    /// Discard only a provisional token retained for this exact request.
    ///
    /// Invalid outcomes must not be able to cancel an unrelated provisional job merely by
    /// returning its token from the wrong handler invocation.
    pub(super) fn discard_provisional_token_for(
        &self,
        token: &DeferredReplyToken,
        request: &Message,
    ) {
        let Some(owner) = token.registry.upgrade() else {
            return;
        };
        if !Arc::ptr_eq(&owner, &self.inner) {
            return;
        }
        let Ok(entry) = owner.entry(token.id) else {
            return;
        };
        if entry.request.header.uuid != request.header.uuid
            || entry.request.header.name != request.header.name
            || entry.request.header.correlation_id != request.header.correlation_id
            || entry.request.header.reply_to != request.header.reply_to
        {
            return;
        }
        if entry
            .state
            .compare_exchange(PROVISIONAL, DISCARDED, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            owner.remove(token.id);
        }
    }

    pub(super) async fn shutdown(&self) {
        if self.inner.shutting_down.swap(true, Ordering::AcqRel) {
            return;
        }
        let entries = match self.inner.entries.lock() {
            Ok(entries) => entries.values().cloned().collect::<Vec<_>>(),
            Err(_) => return,
        };

        for entry in entries {
            if entry
                .state
                .compare_exchange(
                    PROVISIONAL,
                    CANCELLED_ON_SHUTDOWN,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                self.inner.remove(entry.id);
                continue;
            }
            if entry
                .state
                .compare_exchange(OPEN, SETTLING, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                let remaining = entry.expires_at.saturating_duration_since(Instant::now());
                if !remaining.is_zero() {
                    let timeout = remaining.min(SHUTDOWN_CONFIRM_MAX);
                    let reply = self.inner.reply_message(
                        &entry.verb,
                        error_body(
                            ERR_COMPONENT_STOPPING,
                            "the component stopped before the deferred command completed",
                        ),
                    );
                    let _ = self
                        .inner
                        .messaging
                        .reply_confirmed(&entry.request, reply, timeout)
                        .await;
                }
                entry.state.store(CANCELLED_ON_SHUTDOWN, Ordering::Release);
                self.inner.remove(entry.id);
            }
        }
    }
}

impl DeferredReplyToken {
    fn inner(&self) -> Result<Arc<RegistryInner>> {
        self.registry.upgrade().ok_or_else(|| {
            EdgeCommonsError::Command("deferred reply registry no longer exists".to_string())
        })
    }

    /// Activate a provisional token after the application has durably accepted its work.
    pub fn activate(&self) -> Result<()> {
        let inner = self.inner()?;
        if inner.shutting_down.load(Ordering::Acquire) {
            return Err(EdgeCommonsError::Command(
                "cannot activate a deferred token while the inbox is shutting down".to_string(),
            ));
        }
        let entry = inner.entry(self.id)?;
        entry
            .state
            .compare_exchange(PROVISIONAL, OPEN, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|state| {
                EdgeCommonsError::Command(format!(
                    "deferred token activation requires PROVISIONAL state (was {})",
                    state_name(state)
                ))
            })?;
        Ok(())
    }

    /// Discard a provisional token after durable application acceptance failed.
    pub fn discard(&self) -> Result<()> {
        let inner = self.inner()?;
        let entry = inner.entry(self.id)?;
        entry
            .state
            .compare_exchange(PROVISIONAL, DISCARDED, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|state| {
                EdgeCommonsError::Command(format!(
                    "deferred token discard requires PROVISIONAL state (was {})",
                    state_name(state)
                ))
            })?;
        inner.remove(self.id);
        Ok(())
    }

    /// Settle an open token with the standard success wrapper.
    pub async fn settle_success(&self, result: Option<Value>) -> Result<()> {
        let body = json!({ "ok": true, "result": result.unwrap_or_else(|| json!({})) });
        self.inner()?.settle(self.id, body).await
    }

    /// Settle an open token with the standard coded error wrapper.
    pub async fn settle_error(
        &self,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Result<()> {
        let code = code.into();
        self.inner()?
            .settle(self.id, error_body(&code, message))
            .await
    }

    /// Settle from an existing [`CommandError`].
    pub async fn settle_command_error(&self, error: CommandError) -> Result<()> {
        self.settle_error(error.code, error.message).await
    }
}

impl RegistryInner {
    fn entry(&self, id: Uuid) -> Result<Arc<DeferredEntry>> {
        self.entries
            .lock()
            .map_err(|_| {
                EdgeCommonsError::Command("deferred reply registry is poisoned".to_string())
            })?
            .get(&id)
            .cloned()
            .ok_or_else(|| {
                EdgeCommonsError::Command(
                    "deferred reply token is unknown, expired, or discarded".to_string(),
                )
            })
    }

    fn remove(&self, id: Uuid) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.remove(&id);
        }
    }

    fn reply_message(&self, verb: &str, body: Value) -> Message {
        let snapshot = self.config.load_full();
        MessageBuilder::new(verb, CMD_MESSAGE_VERSION)
            .command(body)
            .from_config(&snapshot)
            .build()
    }

    async fn settle(self: Arc<Self>, id: Uuid, body: Value) -> Result<()> {
        let entry = self.entry(id)?;
        entry
            .state
            .compare_exchange(OPEN, SETTLING, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|state| {
                if matches!(state, SETTLING | SETTLED) {
                    EdgeCommonsError::Command(
                        "deferred reply is already being settled or has settled".to_string(),
                    )
                } else {
                    EdgeCommonsError::Command(format!(
                        "deferred settlement requires OPEN state (was {})",
                        state_name(state)
                    ))
                }
            })?;

        let reply = self.reply_message(&entry.verb, body);
        let mut delay = RETRY_MIN;
        let mut last_error = None;
        loop {
            if entry.state.load(Ordering::Acquire) != SETTLING {
                return Err(EdgeCommonsError::Command(format!(
                    "deferred reply expired while settling{}",
                    last_error
                        .as_ref()
                        .map(|error: &String| format!(": {error}"))
                        .unwrap_or_default()
                )));
            }
            let remaining = entry.expires_at.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                self.expire(id);
                return Err(EdgeCommonsError::Command(format!(
                    "deferred reply expired before confirmation{}",
                    last_error
                        .as_ref()
                        .map(|error: &String| format!(": {error}"))
                        .unwrap_or_default()
                )));
            }

            let attempt_timeout = remaining.min(CONFIRM_ATTEMPT_MAX);
            match self
                .messaging
                .reply_confirmed(&entry.request, reply.clone(), attempt_timeout)
                .await
            {
                Ok(()) => {
                    entry
                        .state
                        .compare_exchange(SETTLING, SETTLED, Ordering::AcqRel, Ordering::Acquire)
                        .map_err(|state| {
                            EdgeCommonsError::Command(format!(
                                "deferred reply was confirmed after state changed to {}",
                                state_name(state)
                            ))
                        })?;
                    return Ok(());
                }
                Err(error) => {
                    last_error = Some(error.to_string());
                }
            }

            let remaining = entry.expires_at.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                continue;
            }
            tokio::time::sleep(delay.min(remaining)).await;
            delay = delay.saturating_mul(2).min(RETRY_MAX);
        }
    }

    fn expire(&self, id: Uuid) {
        let entry = match self.entry(id) {
            Ok(entry) => entry,
            Err(_) => return,
        };
        let previous = loop {
            let state = entry.state.load(Ordering::Acquire);
            if matches!(state, SETTLED | DISCARDED | EXPIRED | CANCELLED_ON_SHUTDOWN) {
                break state;
            }
            if entry
                .state
                .compare_exchange(state, EXPIRED, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                break state;
            }
        };
        if matches!(previous, OPEN | SETTLING) {
            tracing::warn!(
                verb = entry.verb,
                prior_state = state_name(previous),
                "deferred command reply expired before settlement"
            );
        }
        self.remove(id);
    }
}
