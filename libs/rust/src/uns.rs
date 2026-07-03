//! # UNS — unified-namespace topic builder + validator
//!
//! **One-liner purpose**: Build, validate, and filter unified-namespace (UNS) topics
//! (`ecv1[/{site}]/{device}/{component}/{instance}/{class}[/{channel…}]`) bound to a
//! component's resolved [`MessageIdentity`], mirroring the Java canonical
//! `com.mbreissi.ggcommons.uns` package (UNS-CANONICAL-DESIGN §2).
//!
//! ## Overview
//! - [`UnsClass`] — the closed class set (`state`/`metric`/`cfg`/`log` are the
//!   library-owned RESERVED classes; `data`/`evt`/`cmd`/`app` are application classes).
//! - [`UnsScope`] — the wildcard scope for [`Uns::filter`] (a `None` field renders `+`).
//! - [`Uns`] — the identity-bound topic builder/validator. Obtain the component-bound
//!   instance via `GgCommons::uns()` (instance `main`) or an instance-bound one via
//!   `GgCommons::instance(id)?.uns()`.
//! - [`reserved_class_of`] — the §4.1 reserved-class publish-guard predicate used by
//!   the messaging service.
//!
//! ## Normative rules (§2.2 — violations are [`GgError::UnsValidation`] with a
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
//! Reply topics (`ggcommons/reply-…`) are non-UNS and never pass through this builder;
//! the guard ignores them because they are not `ecv1/`-rooted (D-U6).
//!
//! ## Usage Example
//! ```
//! use ggcommons::messaging::message::{HierEntry, MessageIdentity};
//! use ggcommons::uns::{Uns, UnsClass, UnsScope};
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

use crate::error::{GgError, Result};
use crate::messaging::message::MessageIdentity;

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

/// Builds a [`GgError::UnsValidation`] with the given code and detail.
fn violation(code: UnsValidationCode, detail: impl Into<String>) -> GgError {
    GgError::UnsValidation { code, detail: detail.into() }
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
        matches!(self, UnsClass::State | UnsClass::Metric | UnsClass::Cfg | UnsClass::Log)
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
        UnsScope { device: Some(device.into()), ..UnsScope::default() }
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
    /// wiring — components obtain bound instances from the `GgCommons` facade
    /// (`gg.uns()` / `gg.instance(id)?.uns()`).
    ///
    /// `include_root` is whether topics/filters carry the first hierarchy value
    /// (`site`) between the [`Self::ROOT`] root and the device (`topic.includeRoot`,
    /// default `false`). Effective only for identities with a multi-level hierarchy
    /// (≥ 2 `hier` entries) — a no-op otherwise (D-U25).
    pub fn new(identity: MessageIdentity, include_root: bool) -> Uns {
        Uns { identity, include_root }
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
    /// [`GgError::UnsValidation`] on any §2.2 violation.
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
    /// [`GgError::UnsValidation`] on any §2.2 violation.
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
    /// [`GgError::UnsValidation`] on any §2.2 violation.
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
            segments.push(checked_token(&target.hier()[0].value, "site (hier[0]) value")?);
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
                format!("class '{}' requires at least one channel token", cls.token()),
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
    /// [`GgError::UnsValidation`] when a pinned (`Some`) scope field violates the
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
    /// [`GgError::UnsValidation`] with the precise [`UnsValidationCode`] on the
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
/// substring. Also the validation gate for `GgCommons::instance(id)` instance tokens.
/// If anyone later tightens the sanitizer, this rule must tighten with it (and vice
/// versa).
///
/// # Errors
/// [`GgError::UnsValidation`] with `EMPTY_TOKEN` / `BAD_CHAR` / `TRAVERSAL`.
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
            format!("topic is {bytes} UTF-8 bytes (max {})", Uns::MAX_TOPIC_UTF8_BYTES),
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
/// topics pass untouched (`ggcommons/reply-…`, `cloudwatch/metric/put`, foreign MQTT
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
        let bytes = std::fs::read(dir.join(file))
            .unwrap_or_else(|e| panic!("failed to read {file}: {e}"));
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
        let levels = input["hierarchyLevels"].as_array().expect("hierarchyLevels");
        let values = input["identityValues"].as_object().expect("identityValues");
        let hier: Vec<HierEntry> = levels
            .iter()
            .map(|level| {
                let level = level.as_str().expect("level name");
                let value = values[level].as_str().expect("identity value");
                HierEntry { level: level.to_string(), value: sanitize(value) }
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
                HierEntry { level: "site".into(), value: "dallas".into() },
                HierEntry { level: "device".into(), value: "gw-01".into() },
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
                Err(GgError::UnsValidation { code, .. }) => {
                    assert_eq!(code.as_str(), error_code, "case '{name}': wrong error code");
                }
                Err(other) => panic!("case '{name}': expected UnsValidation[{error_code}], got {other}"),
                Ok(got) => panic!("case '{name}': expected error {error_code}, got Ok({got:?})"),
            }
            return;
        }
        let got = result.unwrap_or_else(|e| panic!("case '{name}': unexpected error {e}"));
        if let Some(topic) = expected.get("topic").and_then(Value::as_str) {
            assert_eq!(got.as_deref(), Some(topic), "case '{name}': topic mismatch");
        } else if let Some(filter) = expected.get("filter").and_then(Value::as_str) {
            assert_eq!(got.as_deref(), Some(filter), "case '{name}': filter mismatch");
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
                site: scope_in.get("site").and_then(Value::as_str).map(str::to_string),
                device: scope_in.get("device").and_then(Value::as_str).map(str::to_string),
                component: scope_in.get("component").and_then(Value::as_str).map(str::to_string),
                instance: scope_in.get("instance").and_then(Value::as_str).map(str::to_string),
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
            let expected = case["expected"]["reserved"].as_bool().expect("expected.reserved");
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
            let rebuilt = MessageBuilder::new(str_field(header, "name"), str_field(header, "version"))
                .uuid(str_field(header, "uuid"))
                .timestamp(str_field(header, "timestamp"))
                .correlation_id(str_field(header, "correlation_id"))
                .identity(identity.clone())
                .payload(envelope["body"].clone())
                .build();
            let rebuilt_value = serde_json::to_value(&rebuilt).expect("serialize rebuilt envelope");
            assert_eq!(&rebuilt_value, envelope, "vector '{name}': envelope mismatch");

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
            let topic = uns.topic_for(&identity, cls, channel).expect("vector topic builds");
            assert_eq!(topic, str_field(vector, "topic"), "vector '{name}': topic mismatch");
        }
        eprintln!("uns-test-vectors envelopes.json: {} golden envelopes OK", vectors.len());
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
                .map(|(l, v)| HierEntry { level: (*l).to_string(), value: (*v).to_string() })
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

    fn code(err: GgError) -> UnsValidationCode {
        match err {
            GgError::UnsValidation { code, .. } => code,
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
        assert_eq!(uns.topic(UnsClass::State).unwrap(), "ecv1/gw-01/opcua-adapter/main/state");
        assert_eq!(
            uns.topic_with_channel(UnsClass::Cmd, "sb/status").unwrap(),
            "ecv1/gw-01/opcua-adapter/main/cmd/sb/status"
        );
    }

    #[test]
    fn include_root_applies_only_with_multi_level_hierarchy() {
        // D-U25: single-level + includeRoot is a no-op.
        let uns = Uns::new(single(), true);
        assert_eq!(uns.topic(UnsClass::State).unwrap(), "ecv1/gw-01/opcua-adapter/main/state");
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
            uns.topic_for(&peer, UnsClass::Cmd, Some("set-config")).unwrap(),
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
        assert_eq!(code(uns.topic(UnsClass::Data).unwrap_err()), UnsValidationCode::ChannelRequired);
        // An empty channel means "no channel".
        assert_eq!(
            code(uns.topic_with_channel(UnsClass::Data, "").unwrap_err()),
            UnsValidationCode::ChannelRequired
        );
        assert_eq!(uns.topic_with_channel(UnsClass::Cfg, "").unwrap(), "ecv1/gw-01/opcua-adapter/main/cfg");
    }

    #[test]
    fn token_rule_rejects_bad_tokens() {
        assert_eq!(code(check_token("", "t").unwrap_err()), UnsValidationCode::EmptyToken);
        assert_eq!(code(check_token("a+b", "t").unwrap_err()), UnsValidationCode::BadChar);
        assert_eq!(code(check_token("a#b", "t").unwrap_err()), UnsValidationCode::BadChar);
        assert_eq!(code(check_token("a\\b", "t").unwrap_err()), UnsValidationCode::BadChar);
        assert_eq!(code(check_token("a\u{0001}b", "t").unwrap_err()), UnsValidationCode::BadChar);
        assert_eq!(code(check_token("a\u{007f}b", "t").unwrap_err()), UnsValidationCode::BadChar);
        // D-U26: C1 controls (U+0080-U+009F) are rejected too.
        assert_eq!(code(check_token("a\u{0085}b", "t").unwrap_err()), UnsValidationCode::BadChar);
        assert_eq!(code(check_token("a..b", "t").unwrap_err()), UnsValidationCode::Traversal);
        // Dots and spaces are legal (D5: literal-within-a-level; sanitized values pass).
        assert!(check_token("v1.2", "t").is_ok());
        assert!(check_token("gw 01", "t").is_ok());
    }

    #[test]
    fn depth_budget_is_three_rootless_and_two_rooted() {
        let uns = Uns::new(single(), false);
        assert!(uns.topic_with_channel(UnsClass::Data, "a/b/c").is_ok());
        assert_eq!(
            code(uns.topic_with_channel(UnsClass::Data, "a/b/c/d").unwrap_err()),
            UnsValidationCode::DepthExceeded
        );
        let rooted = Uns::new(multi(), true);
        assert!(rooted.topic_with_channel(UnsClass::Data, "a/b").is_ok());
        assert_eq!(
            code(rooted.topic_with_channel(UnsClass::Data, "a/b/c").unwrap_err()),
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
            code(uns.topic_with_channel(UnsClass::Data, &over_channel).unwrap_err()),
            UnsValidationCode::LengthExceeded
        );
    }

    #[test]
    fn filters_render_wildcards_and_channel_hash() {
        let uns = Uns::new(single(), false);
        assert_eq!(uns.filter(UnsClass::Data, &UnsScope::all()).unwrap(), "ecv1/+/+/+/data/#");
        assert_eq!(uns.filter(UnsClass::State, &UnsScope::all()).unwrap(), "ecv1/+/+/+/state");
        assert_eq!(
            uns.filter(UnsClass::Evt, &UnsScope::component("gw-01", "opcua-adapter")).unwrap(),
            "ecv1/gw-01/opcua-adapter/+/evt/#"
        );
        assert_eq!(
            uns.filter(UnsClass::Cmd, &UnsScope::instance("gw-01", "opcua-adapter", "kep1"))
                .unwrap(),
            "ecv1/gw-01/opcua-adapter/kep1/cmd/#"
        );
        // Rooted: the site position exists (and can be pinned).
        let rooted = Uns::new(multi(), true);
        assert_eq!(rooted.filter(UnsClass::Data, &UnsScope::all()).unwrap(), "ecv1/+/+/+/+/data/#");
        assert_eq!(
            rooted
                .filter(UnsClass::Data, &UnsScope::device("gw-01").with_site("dallas"))
                .unwrap(),
            "ecv1/dallas/gw-01/+/+/data/#"
        );
        // Rootless ignores a pinned site (no site position exists).
        assert_eq!(
            uns.filter(UnsClass::Data, &UnsScope::all().with_site("dallas")).unwrap(),
            "ecv1/+/+/+/data/#"
        );
        // A pinned field still passes the token rule.
        assert_eq!(
            code(uns.filter(UnsClass::Data, &UnsScope::device("gw+01")).unwrap_err()),
            UnsValidationCode::BadChar
        );
    }

    #[test]
    fn validate_accepts_good_topics_and_pins_codes() {
        let uns = Uns::new(multi(), false);
        assert!(uns.validate("ecv1/gw-01/opcua-adapter/main/state").is_ok());
        assert!(uns.validate("ecv1/gw-01/opcua-adapter/main/cmd/sb/status").is_ok());
        assert_eq!(code(uns.validate("").unwrap_err()), UnsValidationCode::EmptyToken);
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
        assert_eq!(code(uns.validate("ecv1/d/c/i").unwrap_err()), UnsValidationCode::BadClass);
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
        assert!(rooted.validate("ecv1/dallas/gw-01/opcua-adapter/main/state").is_ok());
        assert_eq!(
            code(rooted.validate("ecv1/gw-01/opcua-adapter/main/state").unwrap_err()),
            UnsValidationCode::BadClass
        );
        // Single-level + includeRoot is a no-op (D-U25): still the rootless positions.
        let noop = Uns::new(single(), true);
        assert!(noop.validate("ecv1/gw-01/opcua-adapter/main/state").is_ok());
    }

    #[test]
    fn guard_predicate_matches_d_u24() {
        assert_eq!(reserved_class_of("ecv1/d/c/i/state", false), Some(UnsClass::State));
        assert_eq!(reserved_class_of("ecv1/d/c/i/metric/cpu", false), Some(UnsClass::Metric));
        assert_eq!(reserved_class_of("ecv1/d/c/i/cfg", false), Some(UnsClass::Cfg));
        assert_eq!(reserved_class_of("ecv1/d/c/i/log/tail", false), Some(UnsClass::Log));
        assert_eq!(reserved_class_of("ecv1/d/c/i/data/temp", false), None);
        // Position 4 is checked even under root mode.
        assert_eq!(reserved_class_of("ecv1/d/c/i/cfg", true), Some(UnsClass::Cfg));
        // Position 5 only when includeRoot is effective.
        assert_eq!(reserved_class_of("ecv1/s/d/c/i/state", true), Some(UnsClass::State));
        assert_eq!(reserved_class_of("ecv1/s/d/c/i/state", false), None);
        // app/state at position 5 rootless is a legit channel.
        assert_eq!(reserved_class_of("ecv1/d/c/i/app/state", false), None);
        assert_eq!(reserved_class_of("ecv1/s/d/c/i/app/state", true), None);
        // Non-ecv1 topics always pass.
        assert_eq!(reserved_class_of("ggcommons/reply-42", false), None);
        assert_eq!(reserved_class_of("cloudwatch/metric/put", false), None);
        // A root-PREFIXED but different first token passes.
        assert_eq!(reserved_class_of("ecv1x/d/c/i/state", false), None);
        // Too-short topics pass.
        assert_eq!(reserved_class_of("ecv1/d/state", false), None);
    }
}
