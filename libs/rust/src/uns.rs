//! # UNS — unified-namespace topic builder + validator
//!
//! **One-liner purpose**: Build, validate, and filter unified-namespace (UNS) topics
//! (`ecv1[/{site}]/{device}/{component}/{instance}/{class}[/{channel…}]`) bound to a
//! component's resolved [`MessageIdentity`], mirroring the Java canonical
//! `com.mbreissi.edgecommons.uns` package (UNS-CANONICAL-DESIGN §2).
//!
//! ## Overview
//! - [`UnsClass`] — the closed class set (`state`/`metric`/`cfg`/`log` are the
//!   library-owned RESERVED classes; `data`/`evt`/`cmd`/`app` are application classes).
//! - [`UnsScope`] — the wildcard scope for [`Uns::filter`] (a `None` field renders `+`).
//! - [`Uns`] — the identity-bound topic builder/validator. Obtain the component-bound
//!   instance via `EdgeCommons::uns()` (instance `main`) or an instance-bound one via
//!   `EdgeCommons::instance(id)?.uns()`.
//! - [`reserved_class_of`] — the §4.1 reserved-class publish-guard predicate used by
//!   the messaging service.
//!
//! ## Normative rules (§2.2 — violations are [`EdgeCommonsError::UnsValidation`] with a
//! machine-readable [`UnsValidationCode`])
//! 1. **Token rule** — identical to the config template sanitizer's blacklist
//!    ([`crate::config::template::sanitize`]), so "sanitized ⇒ valid" is a true
//!    equivalence (D-U26): a token is non-empty, contains no `/ + # \`, no control
//!    characters (Unicode `Cc`: C0 U+0000–U+001F, U+007F, **and C1 U+0080–U+009F**),
//!    and no `..` substring. Dots are legal (a literal within a level).
//! 2. **Depth guard** — at most [`Uns::MAX_TOPIC_SLASHES`] `/` separators (AWS IoT
//!    Core's 8-level limit): the channel budget is 3 tokens rootless / 2 rooted.
//! 3. **Length** — at most [`Uns::MAX_TOPIC_UTF8_BYTES`] UTF-8 bytes.
//! 4. **Class rules** — leaf classes (`state`, `cfg`) forbid a channel; every other
//!    class requires at least one channel token.
//!
//! The optional `site` position (the first hierarchy value) is emitted only when
//! `topic.includeRoot` is `true` **and** the identity carries a multi-level hierarchy
//! (≥ 2 `hier` entries — D-U25): with a single-level hierarchy `hier[0]` *is* the
//! device, so includeRoot is a no-op (prepending would duplicate the device).
//!
//! Reply topics (`edgecommons/reply-…`) are non-UNS and never pass through this builder;
//! the guard ignores them because they are not `ecv1/`-rooted (D-U6).
//!
//! ## Usage Example
//! ```
//! use edgecommons::messaging::message::{HierEntry, MessageIdentity};
//! use edgecommons::uns::{Uns, UnsClass, UnsScope};
//!
//! let identity = MessageIdentity::new(
//!     vec![HierEntry { level: "device".into(), value: "gw-01".into() }],
//!     "opcua-adapter",
//!     None,
//! ).unwrap();
//! let uns = Uns::new(identity, false);
//! assert_eq!(uns.topic(UnsClass::State).unwrap(), "ecv1/gw-01/opcua-adapter/main/state");
//! assert_eq!(
//!     uns.topic_with_channel(UnsClass::Data, "temp").unwrap(),
//!     "ecv1/gw-01/opcua-adapter/main/data/temp"
//! );
//! assert_eq!(uns.filter(UnsClass::Data, &UnsScope::all()).unwrap(), "ecv1/+/+/+/data/#");
//! ```
//!
//! ## Related Modules
//! - [`crate::messaging::message`] — the [`MessageIdentity`] envelope element.
//! - [`crate::config`] — resolves the component identity + `topic.includeRoot`.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rand::Rng;

use crate::error::{EdgeCommonsError, Result};
use crate::messaging::message::{HierEntry, Message, MessageIdentity};
use crate::messaging::{MessagingService, message_handler};

/// The machine-readable UNS validation failure codes (the exact §2.2 set, pinned in
/// `uns-test-vectors/topics.json` so all four languages fail identically).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnsValidationCode {
    /// A topic level / channel token / instance id is empty (or the whole topic is).
    EmptyToken,
    /// A token contains a blacklisted character: `/ + # \` or a control character
    /// (Unicode `Cc` — C0 U+0000–U+001F, U+007F, and C1 U+0080–U+009F; the exact
    /// sanitizer predicate, D-U26).
    BadChar,
    /// A token contains the path-traversal sequence `..`.
    Traversal,
    /// The topic exceeds the IoT Core depth limit (more than 7 `/` separators / 8 levels).
    DepthExceeded,
    /// The topic exceeds the IoT Core publish limit of 256 UTF-8 bytes.
    LengthExceeded,
    /// A channel was supplied for a leaf class (`state`, `cfg`).
    ChannelOnLeaf,
    /// No channel was supplied for a channeled (non-leaf) class.
    ChannelRequired,
    /// The topic does not start with the UNS root literal [`Uns::ROOT`].
    BadRoot,
    /// The class position holds no token or a token outside the closed [`UnsClass`] set.
    BadClass,
    /// [`Uns::validate`] accepts only concrete topics: `+`/`#` are rejected.
    WildcardInTopic,
}

impl UnsValidationCode {
    /// The pinned wire spelling of this code (`EMPTY_TOKEN`, `BAD_CHAR`, …) — the
    /// exact strings in `uns-test-vectors/topics.json`.
    pub const fn as_str(self) -> &'static str {
        match self {
            UnsValidationCode::EmptyToken => "EMPTY_TOKEN",
            UnsValidationCode::BadChar => "BAD_CHAR",
            UnsValidationCode::Traversal => "TRAVERSAL",
            UnsValidationCode::DepthExceeded => "DEPTH_EXCEEDED",
            UnsValidationCode::LengthExceeded => "LENGTH_EXCEEDED",
            UnsValidationCode::ChannelOnLeaf => "CHANNEL_ON_LEAF",
            UnsValidationCode::ChannelRequired => "CHANNEL_REQUIRED",
            UnsValidationCode::BadRoot => "BAD_ROOT",
            UnsValidationCode::BadClass => "BAD_CLASS",
            UnsValidationCode::WildcardInTopic => "WILDCARD_IN_TOPIC",
        }
    }
}

impl std::fmt::Display for UnsValidationCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Builds a [`EdgeCommonsError::UnsValidation`] with the given code and detail.
fn violation(code: UnsValidationCode, detail: impl Into<String>) -> EdgeCommonsError {
    EdgeCommonsError::UnsValidation {
        code,
        detail: detail.into(),
    }
}

/// The closed UNS class set (UNS-CANONICAL-DESIGN §2.1) — the class topic level of
/// every UNS topic (`ecv1[/{site}]/{device}/{component}/{instance}/{class}[/{channel…}]`).
///
/// Each class is either a **leaf** (the class token is the last topic level — a
/// channel is forbidden) or **channeled** (at least one channel token is REQUIRED).
/// The library-owned publish classes (`state | metric | cfg | log`) are
/// [`reserved`](Self::is_reserved): components must not publish to them directly —
/// the reserved-class publish guard enforces this on every public publish path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnsClass {
    /// Component liveness/state keepalive (library-owned). Leaf.
    State,
    /// Component metrics (library-owned). Channeled.
    Metric,
    /// Effective-configuration announcements (library-owned). Leaf.
    Cfg,
    /// Log tailing (library-owned; publisher lands in a later phase). Channeled.
    Log,
    /// Application telemetry/data. Channeled.
    Data,
    /// Application events. Channeled.
    Evt,
    /// Command inboxes (request/reply verbs). Channeled.
    Cmd,
    /// Free-form application namespace. Channeled.
    App,
}

impl UnsClass {
    /// The wire token — the class topic level exactly as it appears in a topic.
    pub const fn token(self) -> &'static str {
        match self {
            UnsClass::State => "state",
            UnsClass::Metric => "metric",
            UnsClass::Cfg => "cfg",
            UnsClass::Log => "log",
            UnsClass::Data => "data",
            UnsClass::Evt => "evt",
            UnsClass::Cmd => "cmd",
            UnsClass::App => "app",
        }
    }

    /// Leaf semantics: `true` — channel forbidden; `false` — channel REQUIRED.
    pub const fn is_leaf(self) -> bool {
        matches!(self, UnsClass::State | UnsClass::Cfg)
    }

    /// Whether this is a library-owned publish class (`state | metric | cfg | log`).
    pub const fn is_reserved(self) -> bool {
        matches!(
            self,
            UnsClass::State | UnsClass::Metric | UnsClass::Cfg | UnsClass::Log
        )
    }

    /// Resolves a wire token to its class, or `None` when the token is outside the
    /// closed set.
    pub fn from_token(token: &str) -> Option<UnsClass> {
        match token {
            "state" => Some(UnsClass::State),
            "metric" => Some(UnsClass::Metric),
            "cfg" => Some(UnsClass::Cfg),
            "log" => Some(UnsClass::Log),
            "data" => Some(UnsClass::Data),
            "evt" => Some(UnsClass::Evt),
            "cmd" => Some(UnsClass::Cmd),
            "app" => Some(UnsClass::App),
            _ => None,
        }
    }
}

/// The wildcard scope for [`Uns::filter`] (UNS-CANONICAL-DESIGN §2.1).
///
/// A `None` field renders as the MQTT single-level wildcard `+` at that topic
/// position; a `Some` field pins the position to that concrete token. The `site`
/// field is used only when the bound `topic.includeRoot` is effective (the rooted
/// grammar has a site position between the [`Uns::ROOT`] root and the device); it
/// is ignored otherwise.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnsScope {
    /// The first-hierarchy-level value to pin (rooted grammar only), or `None` for `+`.
    pub site: Option<String>,
    /// The device (thing) token to pin, or `None` for `+`.
    pub device: Option<String>,
    /// The component token to pin, or `None` for `+`.
    pub component: Option<String>,
    /// The instance token to pin, or `None` for `+`.
    pub instance: Option<String>,
}

impl UnsScope {
    /// Every position wildcarded — all devices, components and instances.
    pub fn all() -> UnsScope {
        UnsScope::default()
    }

    /// All components/instances on one device.
    pub fn device(device: impl Into<String>) -> UnsScope {
        UnsScope {
            device: Some(device.into()),
            ..UnsScope::default()
        }
    }

    /// All instances of one component on one device.
    pub fn component(device: impl Into<String>, component: impl Into<String>) -> UnsScope {
        UnsScope {
            device: Some(device.into()),
            component: Some(component.into()),
            ..UnsScope::default()
        }
    }

    /// One exact instance of one component on one device.
    pub fn instance(
        device: impl Into<String>,
        component: impl Into<String>,
        instance: impl Into<String>,
    ) -> UnsScope {
        UnsScope {
            site: None,
            device: Some(device.into()),
            component: Some(component.into()),
            instance: Some(instance.into()),
        }
    }

    /// Pins the `site` position (used only under an effective rooted grammar).
    pub fn with_site(mut self, site: impl Into<String>) -> UnsScope {
        self.site = Some(site.into());
        self
    }
}

/// The unified-namespace (UNS) topic builder + validator (UNS-CANONICAL-DESIGN §2),
/// bound to a [`MessageIdentity`] and the component's `topic.includeRoot` setting.
///
/// See the [module docs](self) for the grammar and the normative validation rules.
#[derive(Debug, Clone)]
pub struct Uns {
    identity: MessageIdentity,
    include_root: bool,
}

impl Uns {
    /// The UNS root literal — the first token of every UNS topic.
    pub const ROOT: &'static str = "ecv1";

    /// AWS IoT Core's 8-level topic limit, expressed as the maximum `/` separator count.
    pub const MAX_TOPIC_SLASHES: usize = 7;

    /// AWS IoT Core's topic publish limit in UTF-8 bytes.
    pub const MAX_TOPIC_UTF8_BYTES: usize = 256;

    /// Creates a topic builder bound to an identity and a root mode. Library-internal
    /// wiring — components obtain bound instances from the `EdgeCommons` facade
    /// (`gg.uns()` / `gg.instance(id)?.uns()`).
    ///
    /// `include_root` is whether topics/filters carry the first hierarchy value
    /// (`site`) between the [`Self::ROOT`] root and the device (`topic.includeRoot`,
    /// default `false`). Effective only for identities with a multi-level hierarchy
    /// (≥ 2 `hier` entries) — a no-op otherwise (D-U25).
    pub fn new(identity: MessageIdentity, include_root: bool) -> Uns {
        Uns {
            identity,
            include_root,
        }
    }

    /// Returns the bound identity.
    pub fn identity(&self) -> &MessageIdentity {
        &self.identity
    }

    /// Builds the bound identity's concrete topic for a **leaf** class (`state`,
    /// `cfg`) — or, for a channeled class, fails with `CHANNEL_REQUIRED` (use
    /// [`Self::topic_with_channel`]).
    ///
    /// # Errors
    /// [`EdgeCommonsError::UnsValidation`] on any §2.2 violation.
    pub fn topic(&self, cls: UnsClass) -> Result<String> {
        self.topic_for(&self.identity, cls, None)
    }

    /// Builds the bound identity's concrete topic for a channeled class.
    ///
    /// `channel` is one or more `/`-separated tokens (≤ 3 rootless, ≤ 2 rooted),
    /// e.g. `"temp"` or `"sb/status"`; an empty string means "no channel" (only
    /// legal for leaf classes).
    ///
    /// # Errors
    /// [`EdgeCommonsError::UnsValidation`] on any §2.2 violation.
    pub fn topic_with_channel(&self, cls: UnsClass, channel: &str) -> Result<String> {
        self.topic_for(&self.identity, cls, Some(channel))
    }

    /// Builds a concrete topic for a **peer's** identity — typically a received
    /// message's [`crate::messaging::Message::identity`] — which is how a component
    /// addresses a peer's `cmd` inbox without parsing topics. The target's tokens
    /// pass the same token rule as the bound identity's (a foreign identity with
    /// unsanitized values fails to build, it never produces an unpublishable topic).
    ///
    /// # Errors
    /// [`EdgeCommonsError::UnsValidation`] on any §2.2 violation.
    pub fn topic_for(
        &self,
        target: &MessageIdentity,
        cls: UnsClass,
        channel: Option<&str>,
    ) -> Result<String> {
        // D-U25: the site position exists only for a multi-level hierarchy — with a
        // single-level hierarchy hier[0] IS the device, so prepending it would
        // duplicate the device level.
        let rooted = self.rooted(target);
        let mut segments: Vec<&str> = Vec::with_capacity(Self::MAX_TOPIC_SLASHES + 1);
        segments.push(Self::ROOT);
        if rooted {
            segments.push(checked_token(
                &target.hier()[0].value,
                "site (hier[0]) value",
            )?);
        }
        segments.push(checked_token(target.device(), "device")?);
        segments.push(checked_token(target.component(), "component")?);
        segments.push(checked_token(target.instance(), "instance")?);
        segments.push(cls.token());

        let channel_supplied = channel.is_some_and(|c| !c.is_empty());
        if cls.is_leaf() && channel_supplied {
            return Err(violation(
                UnsValidationCode::ChannelOnLeaf,
                format!(
                    "class '{}' is a leaf class - a channel is forbidden (got '{}')",
                    cls.token(),
                    channel.unwrap_or_default()
                ),
            ));
        }
        if !cls.is_leaf() && !channel_supplied {
            return Err(violation(
                UnsValidationCode::ChannelRequired,
                format!(
                    "class '{}' requires at least one channel token",
                    cls.token()
                ),
            ));
        }
        if channel_supplied {
            for channel_token in channel.unwrap_or_default().split('/') {
                segments.push(checked_token(channel_token, "channel token")?);
            }
        }

        let topic = segments.join("/");
        let slashes = segments.len() - 1;
        if slashes > Self::MAX_TOPIC_SLASHES {
            return Err(violation(
                UnsValidationCode::DepthExceeded,
                format!(
                    "topic '{topic}' has {slashes} '/' separators (max {}; the channel budget is \
                     {} token(s) with an effective root mode of {rooted})",
                    Self::MAX_TOPIC_SLASHES,
                    if rooted { 2 } else { 3 }
                ),
            ));
        }
        check_length(&topic)?;
        Ok(topic)
    }

    /// Builds a subscription filter for a class over a wildcard [`UnsScope`]: `None`
    /// scope fields render as `+`; channeled classes get a trailing `/#` (all
    /// channels); leaf classes end at the class token. The `site` position exists
    /// (and [`UnsScope::site`] is consulted) only when `topic.includeRoot` is `true`
    /// AND the bound identity carries a multi-level hierarchy (D-U25).
    ///
    /// The output is correct by construction and is NOT passed through
    /// [`Self::validate`] (filters legitimately carry wildcards).
    ///
    /// # Errors
    /// [`EdgeCommonsError::UnsValidation`] when a pinned (`Some`) scope field violates the
    /// token rule.
    pub fn filter(&self, cls: UnsClass, scope: &UnsScope) -> Result<String> {
        let mut segments: Vec<&str> = Vec::with_capacity(Self::MAX_TOPIC_SLASHES + 1);
        segments.push(Self::ROOT);
        if self.rooted(&self.identity) {
            segments.push(wildcard_or(scope.site.as_deref(), "site")?);
        }
        segments.push(wildcard_or(scope.device.as_deref(), "device")?);
        segments.push(wildcard_or(scope.component.as_deref(), "component")?);
        segments.push(wildcard_or(scope.instance.as_deref(), "instance")?);
        segments.push(cls.token());
        let filter = segments.join("/");
        Ok(if cls.is_leaf() { filter } else { filter + "/#" })
    }

    /// Validates a **concrete** topic against the full §2.2 grammar under this
    /// instance's root mode: wildcards are rejected (`WILDCARD_IN_TOPIC`); every
    /// token passes the token rule; the first token must be the [`Self::ROOT`]
    /// literal; depth ≤ [`Self::MAX_TOPIC_SLASHES`] separators; length ≤
    /// [`Self::MAX_TOPIC_UTF8_BYTES`] UTF-8 bytes; the class position (5th token
    /// rootless, 6th rooted — the root mode is effective only with a multi-level
    /// bound hierarchy, D-U25) must hold a [`UnsClass`] token; leaf classes must end
    /// at the class token and channeled classes must carry at least one channel token.
    ///
    /// # Errors
    /// [`EdgeCommonsError::UnsValidation`] with the precise [`UnsValidationCode`] on the
    /// first violation found.
    pub fn validate(&self, topic: &str) -> Result<()> {
        if topic.is_empty() {
            return Err(violation(UnsValidationCode::EmptyToken, "topic is empty"));
        }
        if topic.contains('+') || topic.contains('#') {
            return Err(violation(
                UnsValidationCode::WildcardInTopic,
                format!(
                    "validate() accepts only concrete topics - '{topic}' contains an MQTT \
                     wildcard ('+'/'#')"
                ),
            ));
        }
        let tokens: Vec<&str> = topic.split('/').collect();
        for token in &tokens {
            check_token(token, "topic token")?;
        }
        if tokens[0] != Self::ROOT {
            return Err(violation(
                UnsValidationCode::BadRoot,
                format!(
                    "topic '{topic}' must start with the UNS root '{}' (got '{}')",
                    Self::ROOT,
                    tokens[0]
                ),
            ));
        }
        let slashes = tokens.len() - 1;
        if slashes > Self::MAX_TOPIC_SLASHES {
            return Err(violation(
                UnsValidationCode::DepthExceeded,
                format!(
                    "topic '{topic}' has {slashes} '/' separators (max {})",
                    Self::MAX_TOPIC_SLASHES
                ),
            ));
        }
        check_length(topic)?;
        let class_position = if self.rooted(&self.identity) { 5 } else { 4 };
        if tokens.len() <= class_position {
            return Err(violation(
                UnsValidationCode::BadClass,
                format!(
                    "topic '{topic}' has too few levels ({}): the class token is expected at \
                     position {class_position} (effective root mode {})",
                    tokens.len(),
                    self.rooted(&self.identity)
                ),
            ));
        }
        let Some(cls) = UnsClass::from_token(tokens[class_position]) else {
            return Err(violation(
                UnsValidationCode::BadClass,
                format!(
                    "'{}' (position {class_position} of '{topic}') is not a UNS class token",
                    tokens[class_position]
                ),
            ));
        };
        let has_channel = tokens.len() > class_position + 1;
        if cls.is_leaf() && has_channel {
            return Err(violation(
                UnsValidationCode::ChannelOnLeaf,
                format!(
                    "class '{}' is a leaf class - topic '{topic}' must end at the class token",
                    cls.token()
                ),
            ));
        }
        if !cls.is_leaf() && !has_channel {
            return Err(violation(
                UnsValidationCode::ChannelRequired,
                format!(
                    "class '{}' requires at least one channel token - topic '{topic}' ends at \
                     the class token",
                    cls.token()
                ),
            ));
        }
        Ok(())
    }

    /// The effective root mode for an identity (D-U25): `topic.includeRoot` applies
    /// only when the identity carries a multi-level hierarchy — with a single-level
    /// hierarchy `hier[0]` *is* the device, so the site position does not exist and
    /// includeRoot is a no-op (the config layer WARNs once at config time).
    fn rooted(&self, target: &MessageIdentity) -> bool {
        self.include_root && target.hier().len() >= 2
    }
}

/// The §2.2 **token rule** — deliberately the EXACT SAME blacklist as the config
/// template sanitizer ([`crate::config::template::sanitize`]), so "sanitized ⇒ valid"
/// is a true equivalence (D-U26): non-empty, no `/ + # \`, no control characters
/// (Unicode `Cc` — C0 U+0000–U+001F, U+007F, and C1 U+0080–U+009F), no `..`
/// substring. Also the validation gate for `EdgeCommons::instance(id)` instance tokens.
/// If anyone later tightens the sanitizer, this rule must tighten with it (and vice
/// versa).
///
/// # Errors
/// [`EdgeCommonsError::UnsValidation`] with `EMPTY_TOKEN` / `BAD_CHAR` / `TRAVERSAL`.
pub fn check_token(token: &str, what: &str) -> Result<()> {
    if token.is_empty() {
        return Err(violation(
            UnsValidationCode::EmptyToken,
            format!("{what} must be a non-empty token"),
        ));
    }
    for (i, c) in token.char_indices() {
        // D-U26: char::is_control (Unicode Cc) == the sanitizer's control-char
        // predicate (covers C0 U+0000-U+001F, U+007F DEL, and C1 U+0080-U+009F).
        if c == '/' || c == '+' || c == '#' || c == '\\' || c.is_control() {
            return Err(violation(
                UnsValidationCode::BadChar,
                format!(
                    "{what} '{token}' contains a forbidden character at index {i} (no '/', '+', \
                     '#', '\\' or control characters)"
                ),
            ));
        }
    }
    if token.contains("..") {
        return Err(violation(
            UnsValidationCode::Traversal,
            format!("{what} '{token}' contains the traversal sequence '..'"),
        ));
    }
    Ok(())
}

/// [`check_token`] that returns the (valid) token, for inline segment assembly.
fn checked_token<'a>(token: &'a str, what: &str) -> Result<&'a str> {
    check_token(token, what)?;
    Ok(token)
}

/// Renders a scope field: `None` as the `+` wildcard, else the checked token.
fn wildcard_or<'a>(value: Option<&'a str>, what: &str) -> Result<&'a str> {
    match value {
        None => Ok("+"),
        Some(token) => checked_token(token, what),
    }
}

/// Enforces the [`Uns::MAX_TOPIC_UTF8_BYTES`] topic length limit.
fn check_length(topic: &str) -> Result<()> {
    let bytes = topic.len(); // &str is UTF-8: len() IS the UTF-8 byte count.
    if bytes > Uns::MAX_TOPIC_UTF8_BYTES {
        return Err(violation(
            UnsValidationCode::LengthExceeded,
            format!(
                "topic is {bytes} UTF-8 bytes (max {})",
                Uns::MAX_TOPIC_UTF8_BYTES
            ),
        ));
    }
    Ok(())
}

/// The §4.1 reserved-class publish-guard predicate (D-U24): the reserved [`UnsClass`]
/// a client-chosen topic targets, or `None` when the topic is allowed.
///
/// Reserved iff the topic is `ecv1`-rooted and the class token at topic level 4
/// (0-based — the rootless grammar `ecv1/{device}/{component}/{instance}/{class}`) —
/// or at level 5, **only when this component's effective `topic.includeRoot` is
/// true** (D-U27: `includeRoot && hier.len() >= 2`; checking position 5
/// unconditionally would false-positive on legitimate app channels like
/// `ecv1/d/c/i/app/state`) — is one of `state | metric | cfg | log`. Non-`ecv1`
/// topics pass untouched (`edgecommons/reply-…`, `cloudwatch/metric/put`, foreign MQTT
/// bridging). `subscribe*` is never guarded (consumers must read reserved classes).
pub fn reserved_class_of(topic: &str, include_root: bool) -> Option<UnsClass> {
    if !topic.starts_with(Uns::ROOT) {
        return None;
    }
    let tokens: Vec<&str> = topic.split('/').collect();
    if tokens[0] != Uns::ROOT {
        return None;
    }
    if tokens.len() >= 5 {
        if let Some(cls) = UnsClass::from_token(tokens[4]) {
            if cls.is_reserved() {
                return Some(cls);
            }
        }
    }
    if include_root && tokens.len() >= 6 {
        if let Some(cls) = UnsClass::from_token(tokens[5]) {
            if cls.is_reserved() {
                return Some(cls);
            }
        }
    }
    None
}

/// Cross-language conformance against `uns-test-vectors/` (the loader side of the
/// vault-test-vectors pattern): every `topics.json` build/validate/filter case must
/// match byte-for-byte (or by error code), every guard case must match the reserved
/// predicate, and every `envelopes.json` golden envelope must (1) structurally equal
/// a rebuild through the message builder and (2) reproduce its topic byte-for-byte.
/// Existence-guarded: skipped when the vectors directory is absent (e.g. a crate
/// checkout outside the monorepo).
#[cfg(test)]
mod vector_tests {
    use super::*;
    use crate::config::template::sanitize;
    use crate::messaging::message::{HierEntry, MessageBuilder, MessageIdentity};
    use serde_json::Value;

    /// The vectors directory, or `None` (skip) when absent.
    fn vectors_dir() -> Option<std::path::PathBuf> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../uns-test-vectors");
        if dir.is_dir() {
            Some(dir)
        } else {
            eprintln!("uns-test-vectors/ not found; skipping cross-language conformance vectors");
            None
        }
    }

    fn load(dir: &std::path::Path, file: &str) -> Value {
        let bytes =
            std::fs::read(dir.join(file)).unwrap_or_else(|e| panic!("failed to read {file}: {e}"));
        // Some inputs deliberately contain raw C1 control bytes — parse as JSON,
        // do not preprocess.
        serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("{file} is not valid JSON: {e}"))
    }

    fn str_field<'a>(case: &'a Value, key: &str) -> &'a str {
        case.get(key)
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("missing string '{key}' in {case}"))
    }

    /// The identity a `build` case describes: `hierarchyLevels[i]` paired with
    /// `identityValues[<level>]`, values + component through the SANITIZER first
    /// (the config identity-resolution path — pins the D-U26 "sanitized ⇒ valid"
    /// equivalence); `instance` verbatim (a validated token, never sanitized).
    fn case_identity(input: &Value) -> MessageIdentity {
        let levels = input["hierarchyLevels"]
            .as_array()
            .expect("hierarchyLevels");
        let values = input["identityValues"].as_object().expect("identityValues");
        let hier: Vec<HierEntry> = levels
            .iter()
            .map(|level| {
                let level = level.as_str().expect("level name");
                let value = values[level].as_str().expect("identity value");
                HierEntry {
                    level: level.to_string(),
                    value: sanitize(value),
                }
            })
            .collect();
        MessageIdentity::new(
            hier,
            sanitize(str_field(input, "component")),
            Some(str_field(input, "instance").to_string()),
        )
        .expect("vector identity constructs")
    }

    /// A multi-level identity to bind the validator/filter to, so the case's
    /// `includeRoot` input IS the effective root mode (README: D-U25 makes
    /// includeRoot a no-op for single-level hierarchies).
    fn multi_level_identity() -> MessageIdentity {
        MessageIdentity::new(
            vec![
                HierEntry {
                    level: "site".into(),
                    value: "dallas".into(),
                },
                HierEntry {
                    level: "device".into(),
                    value: "gw-01".into(),
                },
            ],
            "opcua-adapter",
            None,
        )
        .unwrap()
    }

    /// Asserts a `Result` against a case's `expected` object: `{topic}`/`{filter}`
    /// byte-for-byte, `{ok:true}`, or `{error: <code>}` compared exactly.
    fn assert_expected(name: &str, expected: &Value, result: Result<Option<String>>) {
        if let Some(error_code) = expected.get("error").and_then(Value::as_str) {
            match result {
                Err(EdgeCommonsError::UnsValidation { code, .. }) => {
                    assert_eq!(code.as_str(), error_code, "case '{name}': wrong error code");
                }
                Err(other) => {
                    panic!("case '{name}': expected UnsValidation[{error_code}], got {other}")
                }
                Ok(got) => panic!("case '{name}': expected error {error_code}, got Ok({got:?})"),
            }
            return;
        }
        let got = result.unwrap_or_else(|e| panic!("case '{name}': unexpected error {e}"));
        if let Some(topic) = expected.get("topic").and_then(Value::as_str) {
            assert_eq!(got.as_deref(), Some(topic), "case '{name}': topic mismatch");
        } else if let Some(filter) = expected.get("filter").and_then(Value::as_str) {
            assert_eq!(
                got.as_deref(),
                Some(filter),
                "case '{name}': filter mismatch"
            );
        } else if expected.get("ok").and_then(Value::as_bool) == Some(true) {
            assert_eq!(got, None, "case '{name}': expected plain ok");
        } else {
            panic!("case '{name}': unrecognized expected shape {expected}");
        }
    }

    #[test]
    fn cross_language_topic_vectors() {
        let Some(dir) = vectors_dir() else { return };
        let doc = load(&dir, "topics.json");

        // ---- build ----
        let build_cases = doc["build"].as_array().expect("build group");
        assert!(!build_cases.is_empty());
        for case in build_cases {
            let name = str_field(case, "name");
            let input = &case["input"];
            let identity = case_identity(input);
            let include_root = input["includeRoot"].as_bool().expect("includeRoot");
            let cls = UnsClass::from_token(str_field(input, "class")).expect("class token");
            let channel = input.get("channel").and_then(Value::as_str);
            let uns = Uns::new(identity.clone(), include_root);
            let result = uns.topic_for(&identity, cls, channel).map(Some);
            assert_expected(name, &case["expected"], result);
        }

        // ---- validate ---- (bound to a multi-level identity so includeRoot is effective)
        let validate_cases = doc["validate"].as_array().expect("validate group");
        assert!(!validate_cases.is_empty());
        for case in validate_cases {
            let name = str_field(case, "name");
            let input = &case["input"];
            let include_root = input["includeRoot"].as_bool().expect("includeRoot");
            let uns = Uns::new(multi_level_identity(), include_root);
            let result = uns.validate(str_field(input, "topic")).map(|()| None);
            assert_expected(name, &case["expected"], result);
        }

        // ---- filter ----
        let filter_cases = doc["filter"].as_array().expect("filter group");
        assert!(!filter_cases.is_empty());
        for case in filter_cases {
            let name = str_field(case, "name");
            let input = &case["input"];
            let include_root = input["includeRoot"].as_bool().expect("includeRoot");
            let scope_in = &input["scope"];
            let scope = UnsScope {
                site: scope_in
                    .get("site")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                device: scope_in
                    .get("device")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                component: scope_in
                    .get("component")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                instance: scope_in
                    .get("instance")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            };
            let cls = UnsClass::from_token(str_field(input, "class")).expect("class token");
            let uns = Uns::new(multi_level_identity(), include_root);
            let result = uns.filter(cls, &scope).map(Some);
            assert_expected(name, &case["expected"], result);
        }

        // ---- guard ----
        let guard_cases = doc["guard"].as_array().expect("guard group");
        assert!(!guard_cases.is_empty());
        for case in guard_cases {
            let name = str_field(case, "name");
            let input = &case["input"];
            let include_root = input["includeRoot"].as_bool().expect("includeRoot");
            let expected = case["expected"]["reserved"]
                .as_bool()
                .expect("expected.reserved");
            let got = reserved_class_of(str_field(input, "topic"), include_root).is_some();
            assert_eq!(got, expected, "guard case '{name}'");
        }

        eprintln!(
            "uns-test-vectors topics.json: {} build, {} validate, {} filter, {} guard cases OK",
            build_cases.len(),
            validate_cases.len(),
            filter_cases.len(),
            guard_cases.len()
        );
    }

    #[test]
    fn cross_language_envelope_vectors() {
        let Some(dir) = vectors_dir() else { return };
        let doc = load(&dir, "envelopes.json");
        let vectors = doc["envelopes"].as_array().expect("envelopes group");
        assert!(!vectors.is_empty());

        for vector in vectors {
            let name = str_field(vector, "name");
            let envelope = &vector["envelope"];
            let header = &envelope["header"];
            // The vector identity, parsed with the lenient wire parser.
            let identity = MessageIdentity::from_wire(&envelope["identity"])
                .unwrap_or_else(|| panic!("vector '{name}': identity must parse"));

            // 1. Rebuild through the builder (pinned uuid/timestamp/correlation_id,
            //    D-U13) and compare STRUCTURALLY (member order not normative, D-U22).
            let rebuilt =
                MessageBuilder::new(str_field(header, "name"), str_field(header, "version"))
                    .uuid(str_field(header, "uuid"))
                    .timestamp(str_field(header, "timestamp"))
                    .correlation_id(str_field(header, "correlation_id"))
                    .identity(identity.clone())
                    .payload(envelope["body"].clone())
                    .build();
            let rebuilt_value = serde_json::to_value(&rebuilt).expect("serialize rebuilt envelope");
            assert_eq!(
                &rebuilt_value, envelope,
                "vector '{name}': envelope mismatch"
            );

            // Both directions: the golden JSON parses back into the same message.
            let parsed = crate::messaging::message::Message::from_slice(
                &serde_json::to_vec(envelope).unwrap(),
            )
            .expect("golden envelope parses");
            assert_eq!(parsed, rebuilt, "vector '{name}': parse-back mismatch");

            // 2. Reproduce the topic byte-for-byte from the vector identity +
            //    class + channel with includeRoot=false (all vectors are rootless).
            let cls = UnsClass::from_token(str_field(vector, "class")).expect("class token");
            let channel = vector.get("channel").and_then(Value::as_str);
            let uns = Uns::new(identity.clone(), false);
            let topic = uns
                .topic_for(&identity, cls, channel)
                .expect("vector topic builds");
            assert_eq!(
                topic,
                str_field(vector, "topic"),
                "vector '{name}': topic mismatch"
            );
        }
        eprintln!(
            "uns-test-vectors envelopes.json: {} golden envelopes OK",
            vectors.len()
        );
    }

    /// Cross-language conformance for the `_bcast` republish listener (DESIGN-uns §9.4; the
    /// contract [`RepublishListener`] implements): the two topics byte-for-byte (built the same
    /// way [`RepublishListener::start`] does — a single-level `[device]` hierarchy under the
    /// reserved `_bcast` pseudo-component), the envelope structure (no `identity`/`tags`/
    /// `reply_to` — fire-and-forget), and the behavior constants ([`JITTER_WINDOW_MS`],
    /// [`COOLDOWN_MS`], and `replyTo: false`).
    #[test]
    fn cross_language_bcast_vectors() {
        let Some(dir) = vectors_dir() else { return };
        let doc = load(&dir, "bcast.json");
        let device = str_field(&doc, "device");
        let commands = doc["commands"].as_array().expect("commands group");
        assert_eq!(commands.len(), 2, "republish-state and republish-cfg");

        for case in commands {
            let name = str_field(case, "name");
            let input = &case["input"];
            assert_eq!(
                str_field(input, "device"),
                device,
                "case '{name}': device mismatch"
            );
            assert_eq!(
                str_field(input, "component"),
                BCAST_COMPONENT,
                "case '{name}': component"
            );
            assert_eq!(
                str_field(input, "instance"),
                MessageIdentity::DEFAULT_INSTANCE,
                "case '{name}': instance"
            );
            assert!(
                !input["includeRoot"].as_bool().expect("includeRoot"),
                "case '{name}': the _bcast topic is always rootless (D-U25)"
            );

            let identity = MessageIdentity::new(
                vec![HierEntry {
                    level: "device".to_string(),
                    value: device.to_string(),
                }],
                BCAST_COMPONENT,
                Some(MessageIdentity::DEFAULT_INSTANCE.to_string()),
            )
            .expect("bcast identity constructs");
            let uns = Uns::new(identity, false);
            let cls = UnsClass::from_token(str_field(input, "class")).expect("class token");
            let topic = uns
                .topic_with_channel(cls, str_field(input, "channel"))
                .expect("bcast topic builds");
            assert_eq!(
                topic,
                str_field(case, "topic"),
                "case '{name}': topic mismatch"
            );

            // Envelope structure (D-U22): no identity/tags/reply_to; empty body; header.name is
            // the verb — fire-and-forget, never replied to.
            let envelope = &case["envelope"];
            assert!(
                envelope.get("identity").is_none(),
                "case '{name}': no identity element"
            );
            assert!(
                envelope.get("tags").is_none(),
                "case '{name}': no tags element"
            );
            assert!(
                envelope["header"].get("reply_to").is_none(),
                "case '{name}': no reply_to (fire-and-forget)"
            );
            assert_eq!(str_field(&envelope["header"], "name"), name);
            assert_eq!(envelope["body"], serde_json::json!({}));
        }

        let behavior = &doc["behavior"];
        assert_eq!(
            behavior["jitterWindowMs"].as_u64().unwrap(),
            JITTER_WINDOW_MS,
            "jitterWindowMs"
        );
        assert_eq!(
            behavior["cooldownMs"].as_u64().unwrap(),
            COOLDOWN_MS,
            "cooldownMs"
        );
        assert!(!behavior["replyTo"].as_bool().unwrap(), "replyTo");

        eprintln!(
            "uns-test-vectors bcast.json: {} commands OK",
            commands.len()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messaging::message::HierEntry;

    fn identity(levels: &[(&str, &str)], component: &str, instance: &str) -> MessageIdentity {
        MessageIdentity::new(
            levels
                .iter()
                .map(|(l, v)| HierEntry {
                    level: (*l).to_string(),
                    value: (*v).to_string(),
                })
                .collect(),
            component,
            Some(instance.to_string()),
        )
        .unwrap()
    }

    fn single() -> MessageIdentity {
        identity(&[("device", "gw-01")], "opcua-adapter", "main")
    }

    fn multi() -> MessageIdentity {
        identity(
            &[("site", "dallas"), ("zone", "zone-3"), ("device", "gw-01")],
            "opcua-adapter",
            "main",
        )
    }

    fn code(err: EdgeCommonsError) -> UnsValidationCode {
        match err {
            EdgeCommonsError::UnsValidation { code, .. } => code,
            other => panic!("expected UnsValidation, got {other}"),
        }
    }

    #[test]
    fn class_tokens_and_shapes() {
        assert_eq!(UnsClass::from_token("state"), Some(UnsClass::State));
        assert_eq!(UnsClass::from_token("bogus"), None);
        assert!(UnsClass::State.is_leaf() && UnsClass::Cfg.is_leaf());
        assert!(!UnsClass::Data.is_leaf());
        assert!(UnsClass::Log.is_reserved());
        assert!(!UnsClass::Cmd.is_reserved());
        assert_eq!(UnsClass::Metric.token(), "metric");
    }

    #[test]
    fn builds_leaf_and_channeled_topics() {
        let uns = Uns::new(single(), false);
        assert_eq!(
            uns.topic(UnsClass::State).unwrap(),
            "ecv1/gw-01/opcua-adapter/main/state"
        );
        assert_eq!(
            uns.topic_with_channel(UnsClass::Cmd, "sb/status").unwrap(),
            "ecv1/gw-01/opcua-adapter/main/cmd/sb/status"
        );
    }

    #[test]
    fn include_root_applies_only_with_multi_level_hierarchy() {
        // D-U25: single-level + includeRoot is a no-op.
        let uns = Uns::new(single(), true);
        assert_eq!(
            uns.topic(UnsClass::State).unwrap(),
            "ecv1/gw-01/opcua-adapter/main/state"
        );
        // Multi-level + includeRoot prepends hier[0] (the site).
        let rooted = Uns::new(multi(), true);
        assert_eq!(
            rooted.topic(UnsClass::State).unwrap(),
            "ecv1/dallas/gw-01/opcua-adapter/main/state"
        );
        // Multi-level rootless uses the last hier value only.
        let rootless = Uns::new(multi(), false);
        assert_eq!(
            rootless.topic(UnsClass::State).unwrap(),
            "ecv1/gw-01/opcua-adapter/main/state"
        );
    }

    #[test]
    fn topic_for_addresses_a_peer_identity() {
        let uns = Uns::new(single(), false);
        let peer = identity(&[("device", "gw-02")], "modbus-adapter", "kep1");
        assert_eq!(
            uns.topic_for(&peer, UnsClass::Cmd, Some("set-config"))
                .unwrap(),
            "ecv1/gw-02/modbus-adapter/kep1/cmd/set-config"
        );
    }

    #[test]
    fn channel_rules() {
        let uns = Uns::new(single(), false);
        assert_eq!(
            code(uns.topic_with_channel(UnsClass::State, "x").unwrap_err()),
            UnsValidationCode::ChannelOnLeaf
        );
        assert_eq!(
            code(uns.topic(UnsClass::Data).unwrap_err()),
            UnsValidationCode::ChannelRequired
        );
        // An empty channel means "no channel".
        assert_eq!(
            code(uns.topic_with_channel(UnsClass::Data, "").unwrap_err()),
            UnsValidationCode::ChannelRequired
        );
        assert_eq!(
            uns.topic_with_channel(UnsClass::Cfg, "").unwrap(),
            "ecv1/gw-01/opcua-adapter/main/cfg"
        );
    }

    #[test]
    fn token_rule_rejects_bad_tokens() {
        assert_eq!(
            code(check_token("", "t").unwrap_err()),
            UnsValidationCode::EmptyToken
        );
        assert_eq!(
            code(check_token("a+b", "t").unwrap_err()),
            UnsValidationCode::BadChar
        );
        assert_eq!(
            code(check_token("a#b", "t").unwrap_err()),
            UnsValidationCode::BadChar
        );
        assert_eq!(
            code(check_token("a\\b", "t").unwrap_err()),
            UnsValidationCode::BadChar
        );
        assert_eq!(
            code(check_token("a\u{0001}b", "t").unwrap_err()),
            UnsValidationCode::BadChar
        );
        assert_eq!(
            code(check_token("a\u{007f}b", "t").unwrap_err()),
            UnsValidationCode::BadChar
        );
        // D-U26: C1 controls (U+0080-U+009F) are rejected too.
        assert_eq!(
            code(check_token("a\u{0085}b", "t").unwrap_err()),
            UnsValidationCode::BadChar
        );
        assert_eq!(
            code(check_token("a..b", "t").unwrap_err()),
            UnsValidationCode::Traversal
        );
        // Dots and spaces are legal (D5: literal-within-a-level; sanitized values pass).
        assert!(check_token("v1.2", "t").is_ok());
        assert!(check_token("gw 01", "t").is_ok());
    }

    #[test]
    fn depth_budget_is_three_rootless_and_two_rooted() {
        let uns = Uns::new(single(), false);
        assert!(uns.topic_with_channel(UnsClass::Data, "a/b/c").is_ok());
        assert_eq!(
            code(
                uns.topic_with_channel(UnsClass::Data, "a/b/c/d")
                    .unwrap_err()
            ),
            UnsValidationCode::DepthExceeded
        );
        let rooted = Uns::new(multi(), true);
        assert!(rooted.topic_with_channel(UnsClass::Data, "a/b").is_ok());
        assert_eq!(
            code(
                rooted
                    .topic_with_channel(UnsClass::Data, "a/b/c")
                    .unwrap_err()
            ),
            UnsValidationCode::DepthExceeded
        );
    }

    #[test]
    fn length_limit_is_256_utf8_bytes() {
        let uns = Uns::new(single(), false);
        // Build a channel that lands exactly at / one over the limit.
        let data_base = "ecv1/gw-01/opcua-adapter/main/data/".len();
        let ok_channel = "x".repeat(Uns::MAX_TOPIC_UTF8_BYTES - data_base);
        let over_channel = "x".repeat(Uns::MAX_TOPIC_UTF8_BYTES - data_base + 1);
        assert!(uns.topic_with_channel(UnsClass::Data, &ok_channel).is_ok());
        assert_eq!(
            code(
                uns.topic_with_channel(UnsClass::Data, &over_channel)
                    .unwrap_err()
            ),
            UnsValidationCode::LengthExceeded
        );
    }

    #[test]
    fn filters_render_wildcards_and_channel_hash() {
        let uns = Uns::new(single(), false);
        assert_eq!(
            uns.filter(UnsClass::Data, &UnsScope::all()).unwrap(),
            "ecv1/+/+/+/data/#"
        );
        assert_eq!(
            uns.filter(UnsClass::State, &UnsScope::all()).unwrap(),
            "ecv1/+/+/+/state"
        );
        assert_eq!(
            uns.filter(
                UnsClass::Evt,
                &UnsScope::component("gw-01", "opcua-adapter")
            )
            .unwrap(),
            "ecv1/gw-01/opcua-adapter/+/evt/#"
        );
        assert_eq!(
            uns.filter(
                UnsClass::Cmd,
                &UnsScope::instance("gw-01", "opcua-adapter", "kep1")
            )
            .unwrap(),
            "ecv1/gw-01/opcua-adapter/kep1/cmd/#"
        );
        // Rooted: the site position exists (and can be pinned).
        let rooted = Uns::new(multi(), true);
        assert_eq!(
            rooted.filter(UnsClass::Data, &UnsScope::all()).unwrap(),
            "ecv1/+/+/+/+/data/#"
        );
        assert_eq!(
            rooted
                .filter(
                    UnsClass::Data,
                    &UnsScope::device("gw-01").with_site("dallas")
                )
                .unwrap(),
            "ecv1/dallas/gw-01/+/+/data/#"
        );
        // Rootless ignores a pinned site (no site position exists).
        assert_eq!(
            uns.filter(UnsClass::Data, &UnsScope::all().with_site("dallas"))
                .unwrap(),
            "ecv1/+/+/+/data/#"
        );
        // A pinned field still passes the token rule.
        assert_eq!(
            code(
                uns.filter(UnsClass::Data, &UnsScope::device("gw+01"))
                    .unwrap_err()
            ),
            UnsValidationCode::BadChar
        );
    }

    #[test]
    fn validate_accepts_good_topics_and_pins_codes() {
        let uns = Uns::new(multi(), false);
        assert!(uns.validate("ecv1/gw-01/opcua-adapter/main/state").is_ok());
        assert!(
            uns.validate("ecv1/gw-01/opcua-adapter/main/cmd/sb/status")
                .is_ok()
        );
        assert_eq!(
            code(uns.validate("").unwrap_err()),
            UnsValidationCode::EmptyToken
        );
        assert_eq!(
            code(uns.validate("ecv1//c/i/state").unwrap_err()),
            UnsValidationCode::EmptyToken
        );
        assert_eq!(
            code(uns.validate("ecv1/+/c/i/state").unwrap_err()),
            UnsValidationCode::WildcardInTopic
        );
        assert_eq!(
            code(uns.validate("notroot/d/c/i/state").unwrap_err()),
            UnsValidationCode::BadRoot
        );
        assert_eq!(
            code(uns.validate("ecv1/d/c/i/bogus/x").unwrap_err()),
            UnsValidationCode::BadClass
        );
        assert_eq!(
            code(uns.validate("ecv1/d/c/i/STATE").unwrap_err()),
            UnsValidationCode::BadClass
        );
        // Too short => the class position is missing => BAD_CLASS (pinned by D-U26 note).
        assert_eq!(
            code(uns.validate("ecv1/d/c/i").unwrap_err()),
            UnsValidationCode::BadClass
        );
        assert_eq!(
            code(uns.validate("ecv1/d/c/i/state/extra").unwrap_err()),
            UnsValidationCode::ChannelOnLeaf
        );
        assert_eq!(
            code(uns.validate("ecv1/d/c/i/data").unwrap_err()),
            UnsValidationCode::ChannelRequired
        );
        assert_eq!(
            code(uns.validate("ecv1/d/c/i/data/a/b/c/d").unwrap_err()),
            UnsValidationCode::DepthExceeded
        );
    }

    #[test]
    fn validate_is_root_mode_sensitive() {
        // Rooted mode expects the class at position 5.
        let rooted = Uns::new(multi(), true);
        assert!(
            rooted
                .validate("ecv1/dallas/gw-01/opcua-adapter/main/state")
                .is_ok()
        );
        assert_eq!(
            code(
                rooted
                    .validate("ecv1/gw-01/opcua-adapter/main/state")
                    .unwrap_err()
            ),
            UnsValidationCode::BadClass
        );
        // Single-level + includeRoot is a no-op (D-U25): still the rootless positions.
        let noop = Uns::new(single(), true);
        assert!(noop.validate("ecv1/gw-01/opcua-adapter/main/state").is_ok());
    }

    #[test]
    fn guard_predicate_matches_d_u24() {
        assert_eq!(
            reserved_class_of("ecv1/d/c/i/state", false),
            Some(UnsClass::State)
        );
        assert_eq!(
            reserved_class_of("ecv1/d/c/i/metric/cpu", false),
            Some(UnsClass::Metric)
        );
        assert_eq!(
            reserved_class_of("ecv1/d/c/i/cfg", false),
            Some(UnsClass::Cfg)
        );
        assert_eq!(
            reserved_class_of("ecv1/d/c/i/log/tail", false),
            Some(UnsClass::Log)
        );
        assert_eq!(reserved_class_of("ecv1/d/c/i/data/temp", false), None);
        // Position 4 is checked even under root mode.
        assert_eq!(
            reserved_class_of("ecv1/d/c/i/cfg", true),
            Some(UnsClass::Cfg)
        );
        // Position 5 only when includeRoot is effective.
        assert_eq!(
            reserved_class_of("ecv1/s/d/c/i/state", true),
            Some(UnsClass::State)
        );
        assert_eq!(reserved_class_of("ecv1/s/d/c/i/state", false), None);
        // app/state at position 5 rootless is a legit channel.
        assert_eq!(reserved_class_of("ecv1/d/c/i/app/state", false), None);
        assert_eq!(reserved_class_of("ecv1/s/d/c/i/app/state", true), None);
        // Non-ecv1 topics always pass.
        assert_eq!(reserved_class_of("edgecommons/reply-42", false), None);
        assert_eq!(reserved_class_of("cloudwatch/metric/put", false), None);
        // A root-PREFIXED but different first token passes.
        assert_eq!(reserved_class_of("ecv1x/d/c/i/state", false), None);
        // Too-short topics pass.
        assert_eq!(reserved_class_of("ecv1/d/state", false), None);
    }
}

// ============================================================================================
// The `_bcast` republish listener (DESIGN-uns §9.3 layer 2 / §9.4; slice G-S1)
// ============================================================================================

/// The reserved broadcast pseudo-component token (UNS-CANONICAL-DESIGN §4.3).
pub(crate) const BCAST_COMPONENT: &str = "_bcast";

/// The re-announce-state broadcast verb (the topic channel token AND the required envelope
/// `header.name` of an accepted trigger).
pub(crate) const REPUBLISH_STATE: &str = "republish-state";

/// The re-announce-effective-config broadcast verb (the topic channel token AND the required
/// envelope `header.name` of an accepted trigger).
pub(crate) const REPUBLISH_CFG: &str = "republish-cfg";

/// The anti-stampede jitter window in ms: an accepted broadcast re-announces after a uniformly
/// random delay in `[0, JITTER_WINDOW_MS]` (DESIGN-uns §9.3: "a random 0 to 2s"). Normative for
/// all four languages; pinned by `uns-test-vectors/bcast.json`.
pub(crate) const JITTER_WINDOW_MS: u64 = 2_000;

/// The per-verb coalescing cooldown in ms, measured from the last ACCEPTED trigger: at most one
/// re-announce per verb per this window, so a looping/duplicated broadcast never amplifies.
/// Normative for all four languages; pinned by `uns-test-vectors/bcast.json`.
pub(crate) const COOLDOWN_MS: u64 = 5_000;

const SUBSCRIBE_MAX_MESSAGES: usize = 8;
const SUBSCRIBE_MAX_CONCURRENCY: usize = 1;
const STATE_IDX: usize = 0;
const CFG_IDX: usize = 1;

/// One out-of-band re-announce action (the `republish-state`/`republish-cfg` verb handlers): an
/// infallible, best-effort async callback. Both wired actions
/// ([`crate::heartbeat::Heartbeat::publish_state_now`],
/// [`crate::config::effective::EffectiveConfigPublisher::publish_now`]) already log-and-swallow
/// their own failures, so [`RepublishListener::fire`] has nothing to catch — unlike the Java
/// canonical's `try/catch` around a throwing `Runnable`.
pub(crate) type RepublishAction =
    Arc<dyn Fn() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// A boxed, already-built future — what a [`Delayer`] schedules.
type BoxFuture = Pin<Box<dyn Future<Output = ()> + Send>>;

/// The delayed-execution seam (the injected-clock discipline — mirrors the `uns-bridge`'s pure
/// `reply.rs`/`policy.rs` modules, which take `now: Instant` as a parameter instead of reading
/// the clock inline): production sleeps-then-runs on a spawned `tokio` task ([`TokioDelayer`]);
/// tests record `(delay, task)` pairs and run them synchronously on demand — no real sleeping,
/// so the jitter/coalescing tests are deterministic and fast.
pub(crate) trait Delayer: Send + Sync {
    /// Schedule `task` to run after `delay_millis`. Returns immediately (does not block).
    fn schedule(&self, delay_millis: u64, task: BoxFuture);
}

/// Production [`Delayer`]: spawns a `tokio` task that sleeps for the jittered delay, then runs
/// the task. Panics inside `task` are isolated to that spawned task (standard `tokio::spawn`
/// behavior) — they cannot crash the component.
struct TokioDelayer;

impl Delayer for TokioDelayer {
    fn schedule(&self, delay_millis: u64, task: BoxFuture) {
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(delay_millis)).await;
            task.await;
        });
    }
}

/// One `_bcast` broadcast verb: the wire verb string (both the topic channel token and the
/// required envelope `header.name` of an accepted trigger) plus the out-of-band re-announce
/// action it fires.
struct RepublishVerb {
    name: &'static str,
    action: RepublishAction,
}

/// Per-verb mutable state, guarded together with the lifecycle flags (mirrors the Java
/// canonical's single monitor over both commands): the resolved topic (set once by
/// [`RepublishListener::start`]) and the pure accept/coalesce gate.
#[derive(Default)]
struct VerbState {
    topic: Option<String>,
    gate: RepublishGate,
}

/// The `_bcast` republish listener's lifecycle flags plus both verbs' mutable state, behind one
/// lock (no `.await` ever happens while holding it).
#[derive(Default)]
struct Inner {
    started: bool,
    closed: bool,
    verbs: [VerbState; 2],
}

/// The pure per-verb accept/coalesce decision (§9.4) — no IO, no clock read: `now` is a
/// parameter, mirroring the `uns-bridge`'s `reply.rs`/`policy.rs` pure-decision modules (e.g.
/// `TokenBucket::try_take(&mut self, now: Instant)`). A trigger is accepted only when no
/// re-announce is already pending AND at least [`COOLDOWN_MS`] have elapsed since the last
/// ACCEPTED trigger (measured from acceptance, not the jittered fire).
#[derive(Debug, Default)]
struct RepublishGate {
    /// A re-announce is scheduled but has not fired yet.
    pending: bool,
    /// The clock time of the last ACCEPTED trigger (the cooldown reference point); `None` until
    /// the first acceptance.
    last_accepted: Option<Instant>,
}

impl RepublishGate {
    /// Accept-or-coalesce: on accept, marks `pending`, records `now` as the new cooldown
    /// reference, and returns `true`; otherwise leaves the state untouched and returns `false`.
    fn accept(&mut self, now: Instant) -> bool {
        if self.pending {
            return false;
        }
        if let Some(last) = self.last_accepted {
            if now.saturating_duration_since(last) < Duration::from_millis(COOLDOWN_MS) {
                return false;
            }
        }
        self.pending = true;
        self.last_accepted = Some(now);
        true
    }

    /// Clears `pending` (called when the jittered re-announce fires, win or lose).
    fn clear_pending(&mut self) {
        self.pending = false;
    }
}

/// The library-owned `_bcast` republish listener — the UNS "late-join lever" (DESIGN-uns §9.3
/// layer 2 / §9.4, DESIGN-uns-bridge §2.5): every component subscribes, on its PRIMARY
/// (local/IPC) connection, the two per-device broadcast command topics for its own device:
///
/// ```text
/// ecv1/{device}/_bcast/main/cmd/republish-state
/// ecv1/{device}/_bcast/main/cmd/republish-cfg
/// ```
///
/// and, on receipt, re-announces out of band: `republish-state` re-emits the heartbeat's
/// `state` keepalive (`{"status":"RUNNING","uptimeSecs":n}`,
/// [`crate::heartbeat::Heartbeat::publish_state_now`]) and `republish-cfg` re-runs the
/// effective-config (`cfg`) publisher
/// ([`crate::config::effective::EffectiveConfigPublisher::publish_now`]). Both actions already
/// publish through the privileged [`crate::messaging::ReservedMessaging`] seam internally — this
/// listener never touches it directly, which is why it is library plumbing: component code
/// cannot reach the reserved `state`/`cfg` classes itself. The `uns-bridge` publishes these
/// broadcasts on every site-connection re-establishment so the site view rehydrates without
/// broker retain; the edge-console uses `republish-cfg` for config review.
///
/// **Normative behavior** (mirrored by the Python/Rust/TS listeners; constants pinned by
/// `uns-test-vectors/bcast.json`):
/// - **Topics** — built through the library topic builder with the reserved `_bcast`
///   pseudo-component identity: single-level hierarchy `[{device: <own device>}]`, component
///   [`BCAST_COMPONENT`], instance `main`, class `cmd`, channel = the verb. Always **rootless**
///   (the identity is single-level, so `includeRoot` is a D-U25 no-op).
/// - **Trigger validation** — the envelope's `header.name` must equal the topic's verb; a
///   missing/mismatched name, a raw (headerless) payload, or any parse anomaly is ignored (DEBUG
///   log) — never crashes the component (see [`Self::handle`]).
/// - **Jitter** — an accepted trigger fires after a uniformly random delay in
///   `[0, JITTER_WINDOW_MS]` ms via the injected [`Delayer`]/clock/jitter seams (no inline
///   `Instant::now()`/`rand::*` call anywhere in this type).
/// - **Coalescing / cooldown (per verb, independent)** — see [`RepublishGate`].
/// - **No config surface** — always on; core plumbing, not a feature toggle.
///
/// Lifecycle: constructed and [`start`](Self::start) by the `EdgeCommons` runtime after
/// initialization completes. Teardown is RAII (`Drop`) — mirrors [`crate::heartbeat::Heartbeat`]
/// — unsubscribing both topics before the messaging transport is torn down.
pub(crate) struct RepublishListener {
    messaging: Arc<dyn MessagingService>,
    verb_defs: [RepublishVerb; 2],
    inner: Mutex<Inner>,
    delayer: Arc<dyn Delayer>,
    clock: Box<dyn Fn() -> Instant + Send + Sync>,
    jitter: Box<dyn Fn(u64) -> u64 + Send + Sync>,
}

impl RepublishListener {
    /// Production wiring: a real `tokio::time::sleep`-based delayer, the monotonic system
    /// clock, and `rand`-backed uniform jitter over `[0, window]`.
    pub(crate) fn new(
        messaging: Arc<dyn MessagingService>,
        state_action: RepublishAction,
        cfg_action: RepublishAction,
    ) -> Arc<RepublishListener> {
        Self::with_seams(
            messaging,
            state_action,
            cfg_action,
            Arc::new(TokioDelayer),
            Box::new(Instant::now),
            Box::new(|window_ms| rand::thread_rng().gen_range(0..=window_ms)),
        )
    }

    /// Full-injection constructor for deterministic tests (fake delayer/clock/jitter) — mirrors
    /// the Java canonical's package-private constructor.
    fn with_seams(
        messaging: Arc<dyn MessagingService>,
        state_action: RepublishAction,
        cfg_action: RepublishAction,
        delayer: Arc<dyn Delayer>,
        clock: Box<dyn Fn() -> Instant + Send + Sync>,
        jitter: Box<dyn Fn(u64) -> u64 + Send + Sync>,
    ) -> Arc<RepublishListener> {
        Arc::new(RepublishListener {
            messaging,
            verb_defs: [
                RepublishVerb {
                    name: REPUBLISH_STATE,
                    action: state_action,
                },
                RepublishVerb {
                    name: REPUBLISH_CFG,
                    action: cfg_action,
                },
            ],
            inner: Mutex::new(Inner::default()),
            delayer,
            clock,
            jitter,
        })
    }

    /// Builds the two own-device `_bcast` topics and subscribes them on the PRIMARY connection.
    /// Best-effort and idempotent: on any topic-build or subscribe failure the listener logs a
    /// WARN and disables itself (returns without setting `started`) — the component must come up
    /// regardless. A second call, or a call after the listener is closed, is a no-op.
    ///
    /// The subscribe handlers hold only a [`std::sync::Weak`] reference back to this listener
    /// (never a strong one) — otherwise the listener could never be dropped while its own
    /// subscriptions are live, and [`Drop::drop`] (which unsubscribes them) would never run.
    pub(crate) async fn start(self: Arc<Self>, device: &str) {
        {
            let inner = self.inner.lock().unwrap();
            if inner.started || inner.closed {
                return;
            }
        }

        // The reserved _bcast pseudo-component pinned to this component's own device. The
        // identity is single-level, so the topic is rootless by construction (D-U25) - the
        // broadcast shape is shared by every component on the device bus, whatever their own
        // hierarchy/root mode (Java canonical: `new Uns(bcast, false)`).
        let identity = match MessageIdentity::new(
            vec![HierEntry {
                level: "device".to_string(),
                value: device.to_string(),
            }],
            BCAST_COMPONENT,
            Some(MessageIdentity::DEFAULT_INSTANCE.to_string()),
        ) {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "failed to build the _bcast identity - the republish listener is disabled"
                );
                return;
            }
        };
        let uns = Uns::new(identity, false);

        let mut topics: Vec<String> = Vec::with_capacity(self.verb_defs.len());
        for verb in &self.verb_defs {
            match uns.topic_with_channel(UnsClass::Cmd, verb.name) {
                Ok(topic) => topics.push(topic),
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        verb = verb.name,
                        "failed to build a _bcast topic - the republish listener is disabled"
                    );
                    return;
                }
            }
        }

        for (i, topic) in topics.iter().enumerate() {
            let weak = Arc::downgrade(&self);
            let handler = message_handler(move |_topic, message| {
                let weak = weak.clone();
                async move {
                    if let Some(listener) = weak.upgrade() {
                        RepublishListener::handle(listener, i, message).await;
                    }
                }
            });
            if let Err(e) = self
                .messaging
                .subscribe(
                    topic,
                    handler,
                    SUBSCRIBE_MAX_MESSAGES,
                    SUBSCRIBE_MAX_CONCURRENCY,
                )
                .await
            {
                tracing::warn!(
                    error = %e,
                    topic,
                    "failed to subscribe a _bcast topic - the republish listener is disabled"
                );
                for prior in &topics[..i] {
                    let _ = self.messaging.unsubscribe(prior).await;
                }
                return;
            }
        }

        let mut inner = self.inner.lock().unwrap();
        for (i, topic) in topics.into_iter().enumerate() {
            inner.verbs[i].topic = Some(topic);
        }
        inner.started = true;
        tracing::info!(
            state_topic = inner.verbs[STATE_IDX].topic.as_deref().unwrap_or(""),
            cfg_topic = inner.verbs[CFG_IDX].topic.as_deref().unwrap_or(""),
            "republish listener subscribed"
        );
    }

    /// One received broadcast: validate the envelope (`header.name` must equal the verb), then
    /// run the accept/coalesce decision. Never panics — a malformed or foreign `_bcast` payload
    /// is ignored at DEBUG. (Rust has no "null message" case — the dispatcher always hands the
    /// handler a parsed [`Message`] — so unlike the Java canonical this needs no null check; a
    /// non-envelope/raw payload is covered by [`Message::is_raw`].)
    async fn handle(listener: Arc<Self>, verb_index: usize, message: Message) {
        let verb_name = listener.verb_defs[verb_index].name;
        if message.is_raw() || message.header.name != verb_name {
            tracing::debug!(
                verb = verb_name,
                "ignoring foreign/malformed _bcast payload"
            );
            return;
        }
        Self::on_broadcast(listener, verb_index).await;
    }

    /// The accept/coalesce decision (per verb): coalesce while a re-announce is pending or
    /// within [`COOLDOWN_MS`] of the last accepted trigger ([`RepublishGate::accept`]);
    /// otherwise accept and schedule the re-announce after a jittered delay in
    /// `[0, JITTER_WINDOW_MS]` ms.
    async fn on_broadcast(listener: Arc<Self>, verb_index: usize) {
        let now = (listener.clock)();
        let verb_name = listener.verb_defs[verb_index].name;
        let accepted = {
            let mut inner = listener.inner.lock().unwrap();
            if inner.closed {
                return;
            }
            inner.verbs[verb_index].gate.accept(now)
        };
        if !accepted {
            tracing::debug!(verb = verb_name, "broadcast coalesced");
            return;
        }
        let delay_millis = (listener.jitter)(JITTER_WINDOW_MS);
        tracing::info!(
            verb = verb_name,
            delay_millis,
            "broadcast accepted; re-announcing"
        );

        // Weak, like the subscribe handler: a pending re-announce must not keep the listener
        // (and therefore its subscriptions) alive past its owner's lifetime.
        let weak = Arc::downgrade(&listener);
        let task: BoxFuture = Box::pin(async move {
            if let Some(listener) = weak.upgrade() {
                RepublishListener::fire(listener, verb_index).await;
            }
        });
        listener.delayer.schedule(delay_millis, task);
    }

    /// The jittered re-announce: best-effort. `pending` is cleared BEFORE the action runs, so a
    /// panicking/misbehaving action cannot wedge the verb — the next broadcast after the
    /// cooldown is still accepted (see the `a_panicking_action_does_not_wedge_the_verb` test).
    async fn fire(listener: Arc<Self>, verb_index: usize) {
        let closed = {
            let mut inner = listener.inner.lock().unwrap();
            inner.verbs[verb_index].gate.clear_pending();
            inner.closed
        };
        if closed {
            return;
        }
        (listener.verb_defs[verb_index].action)().await;
    }

    /// Test-only deterministic teardown: the same unsubscribe-before-exit logic as
    /// [`Drop::drop`], but awaited synchronously (no fire-and-forget spawn), so tests can assert
    /// the post-close state without polling/sleeping. Idempotent. Production teardown is
    /// RAII-only (`Drop`, mirroring [`crate::heartbeat::Heartbeat`]) — this is not part of the
    /// production wiring, hence `#[cfg(test)]`.
    #[cfg(test)]
    pub(crate) async fn close(&self) {
        let topics = self.mark_closed();
        for topic in topics {
            if let Err(e) = self.messaging.unsubscribe(&topic).await {
                tracing::debug!(error = %e, topic, "republish-listener unsubscribe failed");
            }
        }
    }

    /// Marks the listener closed (idempotent) and returns the topics to unsubscribe (empty if
    /// already closed or never started). Shared by [`Self::close`] and [`Drop::drop`].
    fn mark_closed(&self) -> Vec<String> {
        let mut inner = self.inner.lock().unwrap();
        if inner.closed {
            return Vec::new();
        }
        inner.closed = true;
        if inner.started {
            inner.verbs.iter().filter_map(|v| v.topic.clone()).collect()
        } else {
            Vec::new()
        }
    }
}

impl Drop for RepublishListener {
    /// RAII teardown (mirrors [`crate::heartbeat::Heartbeat`]): unsubscribes both `_bcast`
    /// topics — while messaging is still up (the unsubscribe-before-exit rule) — on a spawned
    /// fire-and-forget task, since `Drop` cannot `.await`. A no-op when never started or already
    /// closed, and when no `tokio` runtime is available to spawn on.
    fn drop(&mut self) {
        let topics = self.mark_closed();
        if topics.is_empty() {
            return;
        }
        let messaging = self.messaging.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                for topic in topics {
                    if let Err(e) = messaging.unsubscribe(&topic).await {
                        tracing::debug!(error = %e, topic, "republish-listener unsubscribe failed");
                    }
                }
            });
        }
    }
}

#[cfg(test)]
mod republish_tests {
    use super::*;
    use crate::messaging::message::MessageBuilder;
    use crate::testutil::RecordingMessaging;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    const DEVICE: &str = "test-thing";
    const STATE_TOPIC: &str = "ecv1/test-thing/_bcast/main/cmd/republish-state";
    const CFG_TOPIC: &str = "ecv1/test-thing/_bcast/main/cmd/republish-cfg";

    fn topics() -> std::collections::HashSet<String> {
        std::collections::HashSet::from([STATE_TOPIC.to_string(), CFG_TOPIC.to_string()])
    }

    fn broadcast(verb: &str) -> Message {
        MessageBuilder::new(verb, "1.0")
            .payload(serde_json::json!({}))
            .build()
    }

    // ---------- the pure RepublishGate (no tokio needed) ----------

    #[test]
    fn gate_accepts_first_trigger_and_coalesces_while_pending() {
        let mut gate = RepublishGate::default();
        let t0 = Instant::now();
        assert!(gate.accept(t0), "the first trigger must be accepted");
        assert!(
            !gate.accept(t0),
            "a re-announce is already pending -> coalesce"
        );
    }

    #[test]
    fn gate_coalesces_within_cooldown_and_accepts_at_the_boundary() {
        let mut gate = RepublishGate::default();
        let t0 = Instant::now();
        assert!(gate.accept(t0));
        gate.clear_pending();
        assert!(
            !gate.accept(t0 + Duration::from_millis(COOLDOWN_MS - 1)),
            "just inside the cooldown -> coalesce"
        );
        assert!(
            gate.accept(t0 + Duration::from_millis(COOLDOWN_MS)),
            "at the cooldown boundary -> accept"
        );
    }

    // ---------- the async listener (RecordingDelayer + injected clock/jitter) ----------

    /// Records `(delay_millis, task)` pairs; the test runs tasks synchronously on demand — no
    /// real sleeping (the injected-clock discipline).
    #[derive(Default)]
    struct RecordingDelayer {
        tasks: Mutex<Vec<(u64, BoxFuture)>>,
    }

    impl Delayer for RecordingDelayer {
        fn schedule(&self, delay_millis: u64, task: BoxFuture) {
            self.tasks.lock().unwrap().push((delay_millis, task));
        }
    }

    impl RecordingDelayer {
        fn new() -> Arc<Self> {
            Arc::new(Self::default())
        }

        fn delays(&self) -> Vec<u64> {
            self.tasks.lock().unwrap().iter().map(|(d, _)| *d).collect()
        }

        fn pending(&self) -> usize {
            self.tasks.lock().unwrap().len()
        }

        /// Runs and clears every scheduled task (the "jitter delay elapsed" step).
        async fn run_all(&self) {
            let tasks: Vec<BoxFuture> = {
                let mut guard = self.tasks.lock().unwrap();
                guard.drain(..).map(|(_, t)| t).collect()
            };
            for t in tasks {
                t.await;
            }
        }

        /// Take the single scheduled task (for the panic-isolation test).
        fn take_one(&self) -> BoxFuture {
            self.tasks
                .lock()
                .unwrap()
                .pop()
                .expect("a task was scheduled")
                .1
        }
    }

    /// A fixed-base monotonic clock: `base + offset`, `offset` settable from the test —
    /// mirrors the Java canonical test's `AtomicLong clock`.
    #[derive(Clone)]
    struct FakeClock {
        base: Instant,
        offset_ms: Arc<AtomicU64>,
    }

    impl FakeClock {
        fn new() -> Self {
            Self {
                base: Instant::now(),
                offset_ms: Arc::new(AtomicU64::new(0)),
            }
        }
        fn set(&self, ms: u64) {
            self.offset_ms.store(ms, Ordering::SeqCst);
        }
        fn now(&self) -> Instant {
            self.base + Duration::from_millis(self.offset_ms.load(Ordering::SeqCst))
        }
    }

    /// A [`RepublishAction`] that increments an [`AtomicUsize`] counter.
    fn action_counter() -> (RepublishAction, Arc<AtomicUsize>) {
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_for_action = counter.clone();
        let action: RepublishAction = Arc::new(move || {
            let c = counter_for_action.clone();
            Box::pin(async move {
                c.fetch_add(1, Ordering::SeqCst);
            })
        });
        (action, counter)
    }

    /// Test rig: a listener wired with the recording seams, plus the fakes/counters to assert
    /// against — mirrors the Java canonical test's `@BeforeEach setUp`.
    struct Rig {
        messaging: Arc<RecordingMessaging>,
        listener: Arc<RepublishListener>,
        delayer: Arc<RecordingDelayer>,
        clock: FakeClock,
        jitter_window_seen: Arc<AtomicU64>,
        next_jitter: Arc<AtomicU64>,
        state_calls: Arc<AtomicUsize>,
        cfg_calls: Arc<AtomicUsize>,
    }

    fn rig() -> Rig {
        let messaging = RecordingMessaging::new();
        let delayer = RecordingDelayer::new();
        let clock = FakeClock::new();
        let jitter_window_seen = Arc::new(AtomicU64::new(u64::MAX));
        let next_jitter = Arc::new(AtomicU64::new(0));
        let (state_action, state_calls) = action_counter();
        let (cfg_action, cfg_calls) = action_counter();

        let clock_for_seam = clock.clone();
        let seen = jitter_window_seen.clone();
        let next = next_jitter.clone();
        let listener = RepublishListener::with_seams(
            messaging.clone() as Arc<dyn MessagingService>,
            state_action,
            cfg_action,
            delayer.clone() as Arc<dyn Delayer>,
            Box::new(move || clock_for_seam.now()),
            Box::new(move |window| {
                seen.store(window, Ordering::SeqCst);
                next.load(Ordering::SeqCst)
            }),
        );
        Rig {
            messaging,
            listener,
            delayer,
            clock,
            jitter_window_seen,
            next_jitter,
            state_calls,
            cfg_calls,
        }
    }

    #[tokio::test]
    async fn start_subscribes_both_own_device_bcast_topics() {
        let r = rig();
        r.listener.clone().start(DEVICE).await;
        assert_eq!(
            r.messaging.subscribed_topics(),
            topics(),
            "start() must subscribe exactly the two own-device _bcast republish topics"
        );
    }

    #[tokio::test]
    async fn republish_state_re_emits_the_state_keepalive() {
        let r = rig();
        r.listener.clone().start(DEVICE).await;
        r.messaging
            .simulate_message(STATE_TOPIC, broadcast(REPUBLISH_STATE))
            .await;
        assert_eq!(
            r.state_calls.load(Ordering::SeqCst),
            0,
            "the re-announce must wait for the jitter delay"
        );
        r.delayer.run_all().await;
        assert_eq!(
            r.state_calls.load(Ordering::SeqCst),
            1,
            "republish-state must re-run the state action"
        );
        assert_eq!(
            r.cfg_calls.load(Ordering::SeqCst),
            0,
            "republish-state must not touch the cfg action"
        );
    }

    #[tokio::test]
    async fn republish_cfg_re_runs_the_effective_config_publisher() {
        let r = rig();
        r.listener.clone().start(DEVICE).await;
        r.messaging
            .simulate_message(CFG_TOPIC, broadcast(REPUBLISH_CFG))
            .await;
        r.delayer.run_all().await;
        assert_eq!(
            r.cfg_calls.load(Ordering::SeqCst),
            1,
            "republish-cfg must re-run the cfg action"
        );
        assert_eq!(
            r.state_calls.load(Ordering::SeqCst),
            0,
            "republish-cfg must not touch the state action"
        );
    }

    #[tokio::test]
    async fn jitter_window_is_applied_to_the_scheduled_delay() {
        let r = rig();
        r.next_jitter.store(1234, Ordering::SeqCst);
        r.listener.clone().start(DEVICE).await;
        r.messaging
            .simulate_message(STATE_TOPIC, broadcast(REPUBLISH_STATE))
            .await;
        assert_eq!(
            r.jitter_window_seen.load(Ordering::SeqCst),
            JITTER_WINDOW_MS,
            "the jitter source must be asked for a delay within the normative window"
        );
        assert_eq!(
            r.delayer.delays(),
            vec![1234],
            "the scheduled delay must be exactly the jittered value"
        );
    }

    #[tokio::test]
    async fn broadcasts_coalesce_while_a_re_announce_is_pending() {
        let r = rig();
        r.listener.clone().start(DEVICE).await;
        r.messaging
            .simulate_message(STATE_TOPIC, broadcast(REPUBLISH_STATE))
            .await;
        r.messaging
            .simulate_message(STATE_TOPIC, broadcast(REPUBLISH_STATE))
            .await;
        r.messaging
            .simulate_message(STATE_TOPIC, broadcast(REPUBLISH_STATE))
            .await;
        assert_eq!(
            r.delayer.pending(),
            1,
            "a looping broadcast must coalesce to a single pending re-announce"
        );
        r.delayer.run_all().await;
        assert_eq!(r.state_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn broadcasts_coalesce_within_the_cooldown_and_accept_after_it() {
        let r = rig();
        r.listener.clone().start(DEVICE).await;
        r.messaging
            .simulate_message(STATE_TOPIC, broadcast(REPUBLISH_STATE))
            .await;
        r.delayer.run_all().await; // fired; cooldown runs from the ACCEPTED trigger at t=0

        r.clock.set(COOLDOWN_MS - 1);
        r.messaging
            .simulate_message(STATE_TOPIC, broadcast(REPUBLISH_STATE))
            .await;
        assert_eq!(
            r.delayer.pending(),
            0,
            "a broadcast inside the cooldown must coalesce"
        );
        assert_eq!(r.state_calls.load(Ordering::SeqCst), 1);

        r.clock.set(COOLDOWN_MS);
        r.messaging
            .simulate_message(STATE_TOPIC, broadcast(REPUBLISH_STATE))
            .await;
        assert_eq!(
            r.delayer.pending(),
            1,
            "the cooldown boundary must accept again"
        );
        r.delayer.run_all().await;
        assert_eq!(r.state_calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn the_verbs_rate_limit_independently() {
        let r = rig();
        r.listener.clone().start(DEVICE).await;
        r.messaging
            .simulate_message(STATE_TOPIC, broadcast(REPUBLISH_STATE))
            .await;
        // With a state re-announce pending, a cfg broadcast must still be accepted.
        r.messaging
            .simulate_message(CFG_TOPIC, broadcast(REPUBLISH_CFG))
            .await;
        assert_eq!(
            r.delayer.pending(),
            2,
            "state and cfg coalesce/cooldown independently"
        );
        r.delayer.run_all().await;
        assert_eq!(r.state_calls.load(Ordering::SeqCst), 1);
        assert_eq!(r.cfg_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn foreign_and_malformed_payloads_are_ignored() {
        let r = rig();
        r.listener.clone().start(DEVICE).await;
        // Wrong verb name in the header (foreign command on the topic).
        r.messaging
            .simulate_message(STATE_TOPIC, broadcast("something-else"))
            .await;
        // A raw (headerless) envelope - e.g. junk JSON published on the broadcast topic.
        r.messaging
            .simulate_message(STATE_TOPIC, Message::raw(serde_json::json!({ "x": 1 })))
            .await;
        assert_eq!(
            r.delayer.pending(),
            0,
            "foreign/malformed payloads must never schedule"
        );
        assert_eq!(r.state_calls.load(Ordering::SeqCst), 0);
        assert_eq!(r.cfg_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn invalid_device_disables_the_listener() {
        let r = rig();
        // '/' fails the §2.2 token rule at topic-build time.
        r.listener.clone().start("bad/device").await;
        assert!(
            r.messaging.subscribed_topics().is_empty(),
            "an invalid device disables the listener (WARN + no subscriptions)"
        );
    }

    #[tokio::test]
    async fn a_panicking_action_does_not_wedge_the_verb() {
        let messaging = RecordingMessaging::new();
        let delayer = RecordingDelayer::new();
        let clock = FakeClock::new();
        let (cfg_action, _cfg_calls) = action_counter();
        let panicking_state_action: RepublishAction =
            Arc::new(|| Box::pin(async { panic!("boom") }));
        let clock_for_seam = clock.clone();
        let listener = RepublishListener::with_seams(
            messaging.clone() as Arc<dyn MessagingService>,
            panicking_state_action,
            cfg_action,
            delayer.clone() as Arc<dyn Delayer>,
            Box::new(move || clock_for_seam.now()),
            Box::new(|_| 0),
        );
        listener.clone().start(DEVICE).await;
        messaging
            .simulate_message(STATE_TOPIC, broadcast(REPUBLISH_STATE))
            .await;

        // pending was cleared BEFORE the action ran, so the panic (isolated to its own spawned
        // task, standard tokio behavior) must not wedge the verb.
        let task = delayer.take_one();
        let result = tokio::spawn(task).await;
        assert!(
            result.is_err(),
            "the panic must not escape the spawned task"
        );

        clock.set(COOLDOWN_MS);
        messaging
            .simulate_message(STATE_TOPIC, broadcast(REPUBLISH_STATE))
            .await;
        assert_eq!(
            delayer.pending(),
            1,
            "the verb accepts again after the cooldown"
        );
    }

    #[tokio::test]
    async fn close_unsubscribes_both_topics_and_drops_pending_re_announces() {
        let r = rig();
        r.listener.clone().start(DEVICE).await;
        r.messaging
            .simulate_message(STATE_TOPIC, broadcast(REPUBLISH_STATE))
            .await;
        r.listener.close().await;
        assert!(
            r.messaging.subscribed_topics().is_empty(),
            "close() must unsubscribe both _bcast topics (unsubscribe-before-exit)"
        );
        r.delayer.run_all().await;
        assert_eq!(
            r.state_calls.load(Ordering::SeqCst),
            0,
            "a pending re-announce must not fire after close()"
        );
        // A late broadcast (e.g. a stale queued delivery) is ignored - nothing is subscribed.
        r.messaging
            .simulate_message(STATE_TOPIC, broadcast(REPUBLISH_STATE))
            .await;
        assert_eq!(r.delayer.pending(), 0);
    }

    #[tokio::test]
    async fn close_is_idempotent_and_start_after_close_is_a_no_op() {
        let r = rig();
        r.listener.clone().start(DEVICE).await;
        r.listener.close().await;
        r.listener.close().await; // idempotent, must not panic
        r.listener.clone().start(DEVICE).await; // closed -> must not resubscribe
        assert!(r.messaging.subscribed_topics().is_empty());
    }

    #[tokio::test]
    async fn start_is_idempotent() {
        let r = rig();
        r.listener.clone().start(DEVICE).await;
        r.listener.clone().start(DEVICE).await;
        assert_eq!(r.messaging.subscribed_topics(), topics());
        r.messaging
            .simulate_message(STATE_TOPIC, broadcast(REPUBLISH_STATE))
            .await;
        assert_eq!(
            r.delayer.pending(),
            1,
            "a double start must not double-schedule"
        );
    }

    #[tokio::test]
    async fn drop_unsubscribes_both_topics_as_a_raii_fallback() {
        // Mirrors the Java canonical's close()-on-shutdown, adapted for Rust: production
        // teardown is Drop-only (no explicit close() outside tests).
        let r = rig();
        r.listener.clone().start(DEVICE).await;
        drop(r.listener);
        for _ in 0..50 {
            if r.messaging.subscribed_topics().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            r.messaging.subscribed_topics().is_empty(),
            "Drop must unsubscribe both _bcast topics as the RAII fallback"
        );
    }

    /// The production wiring end to end: the jittered delay is bounded by the window, so the
    /// re-announce lands within it. (Uses real timing, like the Java canonical's
    /// `productionConstructorSchedulesForReal` — the one intentional exception to this module's
    /// otherwise fully deterministic, sleep-free tests.)
    #[tokio::test]
    async fn production_constructor_schedules_for_real() {
        let messaging = RecordingMessaging::new();
        let (state_action, _state_calls) = action_counter();
        let (cfg_action, cfg_calls) = action_counter();
        let listener = RepublishListener::new(
            messaging.clone() as Arc<dyn MessagingService>,
            state_action,
            cfg_action,
        );

        listener.clone().start(DEVICE).await;
        assert_eq!(messaging.subscribed_topics(), topics());
        messaging
            .simulate_message(CFG_TOPIC, broadcast(REPUBLISH_CFG))
            .await;

        let deadline =
            Instant::now() + Duration::from_millis(JITTER_WINDOW_MS) + Duration::from_secs(3);
        while cfg_calls.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(
            cfg_calls.load(Ordering::SeqCst),
            1,
            "the production scheduler must fire the re-announce within the jitter window"
        );

        listener.close().await;
        assert!(messaging.subscribed_topics().is_empty());
    }
}
