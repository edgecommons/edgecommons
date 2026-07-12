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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::RecordingMessaging;

    const REPLY_TO: &str = "edgecommons/reply-deferred-1";

    fn registry() -> (Arc<RecordingMessaging>, DeferredReplyRegistry) {
        let messaging = RecordingMessaging::new();
        let config = Arc::new(ArcSwap::from_pointee(
            Config::from_value("TestComponent", "test-thing", json!({})).unwrap(),
        ));
        let registry = DeferredReplyRegistry::new(messaging.clone(), config);
        (messaging, registry)
    }

    /// A well-formed deferrable request: a verb, a correlation id and a `reply_to`.
    fn request(verb: &str) -> Message {
        MessageBuilder::new(verb, CMD_MESSAGE_VERSION)
            .payload(json!({}))
            .reply_to(REPLY_TO)
            .build()
    }

    /// The single recorded reply body.
    fn only_reply(messaging: &RecordingMessaging) -> Value {
        let replies = messaging.replies();
        assert_eq!(replies.len(), 1, "exactly one reply expected");
        assert_eq!(replies[0].0, REPLY_TO, "the reply must go to reply_to");
        replies[0].1.body.clone()
    }

    // ===================== defer() admission =====================

    #[tokio::test]
    async fn defer_rejects_a_non_positive_lifetime() {
        let (_messaging, registry) = registry();
        let error = registry
            .defer(&request("sb/capture"), Duration::ZERO)
            .unwrap_err();
        assert_eq!(error.code, ERR_INVALID_DEFERRED_TOKEN);
        assert!(error.message.contains("positive lifetime"));
    }

    #[tokio::test]
    async fn defer_rejects_a_lifetime_that_overflows_the_clock() {
        let (_messaging, registry) = registry();
        let error = registry
            .defer(&request("sb/capture"), Duration::MAX)
            .unwrap_err();
        assert_eq!(error.code, ERR_INVALID_DEFERRED_TOKEN);
        assert!(
            error.message.contains("too large"),
            "an unrepresentable deadline must be refused, never silently truncated: {}",
            error.message
        );
    }

    #[test]
    fn defer_requires_an_active_runtime_for_the_expiry_timer() {
        // No `#[tokio::test]`: without a runtime the expiry timer could never be armed, so a
        // token that can never expire must not be handed out.
        let (_messaging, registry) = registry();
        let error = registry
            .defer(&request("sb/capture"), Duration::from_secs(1))
            .unwrap_err();
        assert_eq!(error.code, ERR_INVALID_DEFERRED_TOKEN);
        assert!(error.message.contains("Tokio runtime"));
    }

    #[tokio::test]
    async fn defer_is_refused_once_the_registry_is_shutting_down() {
        let (_messaging, registry) = registry();
        registry.shutdown().await;
        let error = registry
            .defer(&request("sb/capture"), Duration::from_secs(5))
            .unwrap_err();
        assert_eq!(error.code, ERR_COMPONENT_STOPPING);
    }

    #[tokio::test]
    async fn defer_is_bounded_by_the_registry_capacity() {
        let (_messaging, registry) = registry();
        let mut tokens = Vec::with_capacity(DEFERRED_REPLY_CAPACITY);
        for _ in 0..DEFERRED_REPLY_CAPACITY {
            tokens.push(
                registry
                    .defer(&request("sb/capture"), Duration::from_secs(30))
                    .expect("capacity not yet reached"),
            );
        }
        let error = registry
            .defer(&request("sb/capture"), Duration::from_secs(30))
            .unwrap_err();
        assert_eq!(
            error.code, ERR_DEFERRED_CAPACITY,
            "the registry must shed load rather than grow without bound"
        );

        // Retiring one token frees exactly one slot.
        tokens.pop().expect("a token to discard").discard().unwrap();
        registry
            .defer(&request("sb/capture"), Duration::from_secs(30))
            .expect("the discarded token's slot is reusable");
    }

    // ===================== token state machine =====================

    #[tokio::test]
    async fn activate_requires_the_provisional_state() {
        let (_messaging, registry) = registry();
        let token = registry
            .defer(&request("sb/capture"), Duration::from_secs(5))
            .unwrap();
        token.activate().unwrap();
        let error = token.activate().unwrap_err();
        assert!(
            error.to_string().contains("was OPEN"),
            "double activation must be refused: {error}"
        );
    }

    #[tokio::test]
    async fn activate_is_refused_while_the_inbox_is_shutting_down() {
        let (_messaging, registry) = registry();
        let token = registry
            .defer(&request("sb/capture"), Duration::from_secs(5))
            .unwrap();
        registry.shutdown().await;
        let error = token.activate().unwrap_err();
        assert!(
            error.to_string().contains("shutting down"),
            "a token cannot become settleable after the shutdown snapshot: {error}"
        );
    }

    #[tokio::test]
    async fn discard_requires_the_provisional_state() {
        let (_messaging, registry) = registry();
        let token = registry
            .defer(&request("sb/capture"), Duration::from_secs(5))
            .unwrap();
        token.activate().unwrap();
        let error = token.discard().unwrap_err();
        assert!(
            error.to_string().contains("was OPEN"),
            "an activated token must be settled, not discarded: {error}"
        );
    }

    #[tokio::test]
    async fn a_discarded_token_is_forgotten_and_cannot_be_settled() {
        let (messaging, registry) = registry();
        let token = registry
            .defer(&request("sb/capture"), Duration::from_secs(5))
            .unwrap();
        token.discard().unwrap();
        let error = token.settle_success(None).await.unwrap_err();
        assert!(
            error.to_string().contains("unknown, expired, or discarded"),
            "{error}"
        );
        assert!(messaging.replies().is_empty(), "a discard replies nothing");
    }

    #[tokio::test]
    async fn settling_requires_an_activated_token() {
        let (messaging, registry) = registry();
        let token = registry
            .defer(&request("sb/capture"), Duration::from_secs(5))
            .unwrap();
        let error = token.settle_success(None).await.unwrap_err();
        assert!(
            error.to_string().contains("was PROVISIONAL"),
            "work must be durably accepted (activate) before it can reply: {error}"
        );
        assert!(messaging.replies().is_empty());
    }

    #[tokio::test]
    async fn a_cloned_token_cannot_settle_twice() {
        let (messaging, registry) = registry();
        let token = registry
            .defer(&request("sb/capture"), Duration::from_secs(5))
            .unwrap();
        token.activate().unwrap();
        let clone = token.clone();
        token
            .settle_success(Some(json!({ "frames": 3 })))
            .await
            .unwrap();
        let error = clone.settle_success(None).await.unwrap_err();
        assert!(
            error.to_string().contains("unknown, expired, or discarded")
                || error.to_string().contains("already"),
            "cloning a token must not authorize a duplicate reply: {error}"
        );
        assert_eq!(
            only_reply(&messaging),
            json!({ "ok": true, "result": { "frames": 3 } }),
            "exactly one settlement reaches the requester"
        );
    }

    #[tokio::test]
    async fn settle_error_emits_the_standard_coded_error_wrapper() {
        let (messaging, registry) = registry();
        let token = registry
            .defer(&request("sb/capture"), Duration::from_secs(5))
            .unwrap();
        token.activate().unwrap();
        token
            .settle_command_error(CommandError::new("DEVICE_BUSY", "the sensor is capturing"))
            .await
            .unwrap();
        assert_eq!(
            only_reply(&messaging),
            json!({
                "ok": false,
                "error": { "code": "DEVICE_BUSY", "message": "the sensor is capturing" }
            })
        );
    }

    #[tokio::test]
    async fn a_token_is_useless_once_its_registry_is_gone() {
        let (_messaging, registry) = registry();
        let token = registry
            .defer(&request("sb/capture"), Duration::from_secs(5))
            .unwrap();
        drop(registry);
        let error = token.activate().unwrap_err();
        assert!(
            error.to_string().contains("no longer exists"),
            "a token must not outlive its inbox: {error}"
        );
    }

    #[tokio::test]
    async fn the_token_debug_form_leaks_no_request_correlation() {
        let (_messaging, registry) = registry();
        let request = request("sb/capture");
        let token = registry.defer(&request, Duration::from_secs(5)).unwrap();
        let rendered = format!("{token:?}");
        assert_eq!(rendered, "DeferredReplyToken(..)");
        assert!(
            !rendered.contains(&request.header.correlation_id) && !rendered.contains(REPLY_TO),
            "the token is an opaque capability — it must not expose reply_to or correlation"
        );
        assert!(
            format!("{registry:?}").contains(&DEFERRED_REPLY_CAPACITY.to_string()),
            "the registry's Debug form states its bound"
        );
    }

    // ===================== settlement confirmation =====================

    #[tokio::test]
    async fn settlement_retries_until_the_reply_is_confirmed() {
        let (messaging, registry) = registry();
        messaging.fail_next_confirmed(2);
        let token = registry
            .defer(&request("sb/capture"), Duration::from_secs(10))
            .unwrap();
        token.activate().unwrap();
        token.settle_success(None).await.unwrap();
        assert_eq!(
            only_reply(&messaging)["ok"],
            json!(true),
            "a transient publish failure must be retried, not dropped"
        );
    }

    #[tokio::test]
    async fn settlement_gives_up_when_the_token_expires_before_confirmation() {
        let (messaging, registry) = registry();
        messaging.fail_next_confirmed(usize::MAX);
        let token = registry
            .defer(&request("sb/capture"), Duration::from_millis(250))
            .unwrap();
        token.activate().unwrap();
        let error = token.settle_success(None).await.unwrap_err();
        let text = error.to_string();
        assert!(
            text.contains("expired"),
            "settlement is bounded by the token's lifetime: {text}"
        );
        assert!(
            text.contains("simulated confirmed reply failure"),
            "the last transport error must be reported, not swallowed: {text}"
        );
        assert!(messaging.replies().is_empty());
    }

    #[tokio::test]
    async fn an_expired_token_is_evicted_and_can_no_longer_settle() {
        let (messaging, registry) = registry();
        let token = registry
            .defer(&request("sb/capture"), Duration::from_millis(50))
            .unwrap();
        token.activate().unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        let error = token.settle_success(None).await.unwrap_err();
        assert!(
            error.to_string().contains("unknown, expired, or discarded"),
            "the expiry timer must evict the entry, not leak it: {error}"
        );
        assert!(messaging.replies().is_empty());
    }

    // ===================== token/request binding =====================

    #[tokio::test]
    async fn validate_open_token_rejects_a_token_owned_by_another_inbox() {
        let (_m1, first) = registry();
        let (_m2, second) = registry();
        let request = request("sb/capture");
        let token = first.defer(&request, Duration::from_secs(5)).unwrap();
        token.activate().unwrap();

        let error = second.validate_open_token(&token, &request).unwrap_err();
        assert!(
            error.to_string().contains("another command inbox"),
            "a token must only settle through the inbox that minted it: {error}"
        );
    }

    #[tokio::test]
    async fn validate_open_token_rejects_a_token_whose_owner_is_gone() {
        let (_m1, first) = registry();
        let (_m2, second) = registry();
        let request = request("sb/capture");
        let token = first.defer(&request, Duration::from_secs(5)).unwrap();
        drop(first);

        let error = second.validate_open_token(&token, &request).unwrap_err();
        assert!(
            error.to_string().contains("owner no longer exists"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn validate_open_token_requires_the_open_state_and_the_exact_request() {
        let (_messaging, registry) = registry();
        let request = request("sb/capture");
        let token = registry.defer(&request, Duration::from_secs(5)).unwrap();

        let error = registry.validate_open_token(&token, &request).unwrap_err();
        assert!(
            error.to_string().contains("is PROVISIONAL, not OPEN"),
            "an unactivated token is not an acceptable deferred outcome: {error}"
        );

        token.activate().unwrap();
        registry.validate_open_token(&token, &request).unwrap();

        // The same verb and reply_to, but a different message: a handler must not be able to
        // hand back some *other* in-flight request's token.
        let other = self::request("sb/capture");
        let error = registry.validate_open_token(&token, &other).unwrap_err();
        assert!(
            error.to_string().contains("this exact request"),
            "the token is bound to one request, not merely to its verb: {error}"
        );
    }

    #[tokio::test]
    async fn discard_provisional_token_for_only_retires_the_matching_request() {
        let (_messaging, registry) = registry();
        let request = request("sb/capture");
        let token = registry.defer(&request, Duration::from_secs(5)).unwrap();

        // A mismatched request must not be able to cancel this provisional job...
        let other = self::request("sb/capture");
        registry.discard_provisional_token_for(&token, &other);
        registry.validate_open_token(&token, &request).unwrap_err(); // still PROVISIONAL, not gone
        token
            .activate()
            .expect("the token survived the wrong-request discard");

        // ...and an already-activated token is not discardable either.
        let second = registry.defer(&request, Duration::from_secs(5)).unwrap();
        second.activate().unwrap();
        registry.discard_provisional_token_for(&second, &request);
        registry
            .validate_open_token(&second, &request)
            .expect("an OPEN token survives a provisional discard");
    }

    #[tokio::test]
    async fn discard_provisional_token_for_ignores_a_foreign_or_orphaned_token() {
        let (_m1, first) = registry();
        let (_m2, second) = registry();
        let request = request("sb/capture");
        let token = first.defer(&request, Duration::from_secs(5)).unwrap();

        // Another inbox holding the token must not be able to retire it.
        second.discard_provisional_token_for(&token, &request);
        token
            .activate()
            .expect("a foreign registry cannot discard this token");

        // Nor may a token whose owner has been dropped affect anything.
        let orphan = first.defer(&request, Duration::from_secs(5)).unwrap();
        drop(first);
        second.discard_provisional_token_for(&orphan, &request); // must be a silent no-op
    }

    // ===================== shutdown =====================

    #[tokio::test]
    async fn shutdown_cancels_provisional_tokens_and_stop_replies_to_open_ones() {
        let (messaging, registry) = registry();
        let open_request = request("sb/capture");
        let open = registry
            .defer(&open_request, Duration::from_secs(30))
            .unwrap();
        open.activate().unwrap();
        let provisional = registry
            .defer(&request("sb/capture"), Duration::from_secs(30))
            .unwrap();

        registry.shutdown().await;

        assert_eq!(
            only_reply(&messaging),
            json!({
                "ok": false,
                "error": {
                    "code": ERR_COMPONENT_STOPPING,
                    "message": "the component stopped before the deferred command completed"
                }
            }),
            "an accepted (OPEN) deferred command owes the requester a stop reply"
        );
        // Both entries are retired: neither token can settle after shutdown.
        assert!(open.settle_success(None).await.is_err());
        assert!(provisional.activate().is_err());

        registry.shutdown().await; // idempotent
        assert_eq!(
            messaging.replies().len(),
            1,
            "shutdown replies exactly once"
        );
    }

    #[tokio::test]
    async fn shutdown_does_not_reply_for_an_open_token_that_already_expired() {
        let (messaging, registry) = registry();
        let token = registry
            .defer(&request("sb/capture"), Duration::from_millis(60))
            .unwrap();
        token.activate().unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;

        registry.shutdown().await;
        assert!(
            messaging.replies().is_empty(),
            "an already-expired token has no live requester left to answer"
        );
    }
}
