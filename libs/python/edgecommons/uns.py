"""The unified-namespace (UNS) topic builder + validator (UNS-CANONICAL-DESIGN §2).

Grammar (§2.2)::

    ecv1 [/ {site}]? / {device} / {component} / {instance} / {class} [/ {channel...}]

The optional ``site`` position (the first hierarchy value) is emitted only when
``topic.includeRoot`` is true **and** the bound identity carries a multi-level
hierarchy (>= 2 ``hier`` entries — D-U25). With a single-level hierarchy
(``["device"]``) ``hier[0]`` *is* the device, so includeRoot is a no-op (prepending
would duplicate the device: ``ecv1/gw-01/gw-01/...``).

Obtain the component-bound instance via ``gg.uns()`` (instance ``"main"``) or an
instance-bound one via ``gg.instance(id).uns()``.

Normative rules (each violation raises :class:`UnsValidationError` with a
machine-readable ``code``):

1. **Token rule** — identical to the config template sanitizer's blacklist
   (``ConfigManager.sanitize``), so "sanitized => valid" is a true equivalence
   (D-U26): a token is non-empty, contains no ``/ + # \\``, no ISO control
   characters (C0 U+0000–U+001F, U+007F, **and C1 U+0080–U+009F**), and no ``..``
   substring. Dots are legal (a literal within a level).
2. **Depth guard** — at most :data:`Uns.MAX_TOPIC_SLASHES` ``/`` separators total
   (AWS IoT Core's 8-level limit), so the channel budget is 3 tokens rootless /
   2 tokens rooted; enforced at build time.
3. **Length** — at most :data:`Uns.MAX_TOPIC_UTF8_BYTES` UTF-8 bytes total.
4. **Class rules** — leaf classes (``state``, ``cfg``) forbid a channel; every
   other class requires at least one channel token.

Reply topics (``edgecommons/reply-...``) are non-UNS and never pass through this
builder.
"""
from dataclasses import dataclass
from enum import Enum
from typing import Optional

from edgecommons.messaging.identity import MessageIdentity


class UnsValidationError(ValueError):
    """Raised by the :class:`Uns` topic builder/validator when a topic, filter
    component, or token violates the UNS grammar (UNS-CANONICAL-DESIGN §2.2).
    Carries a machine-readable :attr:`code` (the exact §2.2 set, pinned in
    ``uns-test-vectors/topics.json``) so all four languages fail identically."""

    # The machine-readable UNS validation failure codes (§2.2).
    EMPTY_TOKEN = "EMPTY_TOKEN"
    BAD_CHAR = "BAD_CHAR"
    TRAVERSAL = "TRAVERSAL"
    DEPTH_EXCEEDED = "DEPTH_EXCEEDED"
    LENGTH_EXCEEDED = "LENGTH_EXCEEDED"
    CHANNEL_ON_LEAF = "CHANNEL_ON_LEAF"
    CHANNEL_REQUIRED = "CHANNEL_REQUIRED"
    BAD_ROOT = "BAD_ROOT"
    BAD_CLASS = "BAD_CLASS"
    WILDCARD_IN_TOPIC = "WILDCARD_IN_TOPIC"

    def __init__(self, code: str, message: str):
        super().__init__(f"[{code}] {message}")
        self.code = code


class UnsClass(str, Enum):
    """The closed UNS class set (UNS-CANONICAL-DESIGN §2.1) — the class topic level of
    every UNS topic. Each class is either a **leaf** (the class token is the last topic
    level — a channel is forbidden) or **channeled** (at least one channel token is
    REQUIRED after the class). :data:`RESERVED_CLASSES` lists the library-owned publish
    classes (``state | metric | cfg | log``) that components must not publish to
    directly (enforced by the reserved-class publish guard)."""

    STATE = "state"
    METRIC = "metric"
    CFG = "cfg"
    LOG = "log"
    DATA = "data"
    EVT = "evt"
    CMD = "cmd"
    APP = "app"

    @property
    def token(self) -> str:
        """The wire token — the class topic level exactly as it appears in a topic."""
        return self.value

    @property
    def leaf(self) -> bool:
        """Leaf semantics: ``True`` — channel forbidden; ``False`` — channel REQUIRED."""
        return self.value in ("state", "cfg")

    @staticmethod
    def from_token(token: str) -> Optional["UnsClass"]:
        """The class for a wire token, or ``None`` when outside the closed set."""
        try:
            return UnsClass(token)
        except ValueError:
            return None


#: The library-owned publish classes (``state | metric | cfg | log``).
RESERVED_CLASSES = frozenset({UnsClass.STATE, UnsClass.METRIC, UnsClass.CFG, UnsClass.LOG})
# Java-parity alias (UnsClass.RESERVED).
UnsClass.RESERVED = RESERVED_CLASSES


@dataclass(frozen=True)
class UnsScope:
    """The wildcard scope for :meth:`Uns.filter` (UNS-CANONICAL-DESIGN §2.1).

    A ``None`` field renders as the MQTT single-level wildcard ``+`` at that topic
    position; a non-``None`` field pins the position to that concrete token. The
    ``site`` field is used only when the bound ``topic.includeRoot`` is effective (the
    rooted grammar has a site position between the root and the device); it is
    ignored otherwise."""

    site: Optional[str] = None
    device: Optional[str] = None
    component: Optional[str] = None
    instance: Optional[str] = None

    @staticmethod
    def all() -> "UnsScope":
        """Every position wildcarded — all devices, components and instances."""
        return UnsScope()

    @staticmethod
    def for_device(device: str) -> "UnsScope":
        """All components/instances on one device."""
        return UnsScope(device=device)

    @staticmethod
    def for_component(device: str, component: str) -> "UnsScope":
        """All instances of one component on one device."""
        return UnsScope(device=device, component=component)

    @staticmethod
    def for_instance(device: str, component: str, instance: str) -> "UnsScope":
        """One exact instance of one component on one device."""
        return UnsScope(device=device, component=component, instance=instance)


class Uns:
    """The UNS topic builder + validator, bound to a :class:`MessageIdentity` and the
    component's ``topic.includeRoot`` setting."""

    #: The UNS root literal — the first token of every UNS topic.
    ROOT = "ecv1"

    #: AWS IoT Core's 8-level topic limit, as the maximum ``/`` separator count.
    MAX_TOPIC_SLASHES = 7

    #: AWS IoT Core's topic publish limit in UTF-8 bytes.
    MAX_TOPIC_UTF8_BYTES = 256

    def __init__(self, identity: MessageIdentity, include_root: bool):
        """Creates a topic builder bound to an identity and a root mode. Library-internal
        wiring — components obtain bound instances from the ``EdgeCommons`` facade.

        :param identity: the identity whose tokens :meth:`topic` emits (non-``None``)
        :param include_root: whether topics/filters carry the first hierarchy value
            (``site``) between the root and the device (``topic.includeRoot``, default
            false). Effective only for identities with a multi-level hierarchy
            (>= 2 ``hier`` entries) — a no-op otherwise (D-U25)
        """
        if identity is None:
            raise ValueError("identity must not be None")
        self._identity = identity
        self._include_root = bool(include_root)

    def identity(self) -> MessageIdentity:
        """The bound identity."""
        return self._identity

    def topic(self, cls: UnsClass, channel: Optional[str] = None) -> str:
        """Builds the bound identity's concrete topic for a class.

        :param cls: the UNS class (non-``None``)
        :param channel: the channel — one or more ``/``-separated tokens (<= 3 rootless,
            <= 2 rooted), e.g. ``"temp"`` or ``"sb/status"``; ``None``/empty means
            "no channel" (only legal for leaf classes)
        :returns: the concrete topic, e.g. ``ecv1/gw-01/opcua-adapter/main/state``
        :raises UnsValidationError: on any §2.2 violation
        """
        return self.topic_for(self._identity, cls, channel)

    def topic_for(self, target: MessageIdentity, cls: UnsClass,
                  channel: Optional[str] = None) -> str:
        """Builds a concrete topic for a **peer's** identity — typically a received
        message's ``identity`` — which is how a component addresses a peer's ``cmd``
        inbox without parsing topics. The target's tokens pass the same token rule as
        the bound identity's.

        :raises UnsValidationError: on any §2.2 violation
        """
        if target is None:
            raise ValueError("target identity must not be None")
        if cls is None:
            raise ValueError("class must not be None")
        # D-U25: the site position exists only for a multi-level hierarchy — with a
        # single-level hierarchy hier[0] IS the device, so prepending it would
        # duplicate the device level.
        rooted = self._rooted(target)
        segments = [Uns.ROOT]
        if rooted:
            segments.append(_checked_token(target.hier[0].value, "site (hier[0]) value"))
        segments.append(_checked_token(target.device, "device"))
        segments.append(_checked_token(target.component, "component"))
        segments.append(_checked_token(target.instance, "instance"))
        segments.append(cls.token)

        channel_supplied = bool(channel)
        if cls.leaf and channel_supplied:
            raise UnsValidationError(
                UnsValidationError.CHANNEL_ON_LEAF,
                f"class '{cls.token}' is a leaf class - a channel is forbidden"
                f" (got '{channel}')",
            )
        if not cls.leaf and not channel_supplied:
            raise UnsValidationError(
                UnsValidationError.CHANNEL_REQUIRED,
                f"class '{cls.token}' requires at least one channel token",
            )
        if channel_supplied:
            for channel_token in channel.split("/"):
                segments.append(_checked_token(channel_token, "channel token"))

        topic = "/".join(segments)
        slashes = len(segments) - 1
        if slashes > Uns.MAX_TOPIC_SLASHES:
            raise UnsValidationError(
                UnsValidationError.DEPTH_EXCEEDED,
                f"topic '{topic}' has {slashes} '/' separators (max"
                f" {Uns.MAX_TOPIC_SLASHES}; the channel budget is"
                f" {2 if rooted else 3} token(s) with an effective root mode of"
                f" {rooted})",
            )
        _check_length(topic)
        return topic

    def filter(self, cls: UnsClass, scope: UnsScope) -> str:
        """Builds a subscription filter for a class over a wildcard :class:`UnsScope`:
        ``None`` scope fields render as ``+``; channeled classes get a trailing ``/#``
        (all channels); leaf classes end at the class token. The ``site`` position
        exists (and :attr:`UnsScope.site` is consulted) only when ``topic.includeRoot``
        is true AND the bound identity carries a multi-level hierarchy (D-U25).

        The output is correct by construction and is NOT passed through
        :meth:`validate` (filters legitimately carry wildcards).

        :raises UnsValidationError: when a pinned (non-``None``) scope field violates
            the token rule
        """
        if cls is None:
            raise ValueError("class must not be None")
        if scope is None:
            raise ValueError("scope must not be None (use UnsScope.all())")
        segments = [Uns.ROOT]
        if self._rooted(self._identity):
            segments.append(_wildcard_or(scope.site, "site"))
        segments.append(_wildcard_or(scope.device, "device"))
        segments.append(_wildcard_or(scope.component, "component"))
        segments.append(_wildcard_or(scope.instance, "instance"))
        segments.append(cls.token)
        built = "/".join(segments)
        return built if cls.leaf else built + "/#"

    def validate(self, topic: str) -> None:
        """Validates a **concrete** topic against the full §2.2 grammar under this
        instance's root mode: wildcards are rejected (``WILDCARD_IN_TOPIC``); every
        token passes the token rule; the first token must be the ``ecv1`` root literal;
        depth <= 7 separators; length <= 256 UTF-8 bytes; the class position (5th token
        rootless, 6th rooted — the root mode is effective only with a multi-level bound
        hierarchy, D-U25) must hold a :class:`UnsClass` token; leaf classes must end at
        the class token and channeled classes must carry at least one channel token.

        :raises UnsValidationError: with the precise code on the first violation found
        """
        if not topic:
            raise UnsValidationError(UnsValidationError.EMPTY_TOKEN, "topic is null or empty")
        if "+" in topic or "#" in topic:
            raise UnsValidationError(
                UnsValidationError.WILDCARD_IN_TOPIC,
                f"validate() accepts only concrete topics - '{topic}' contains an MQTT"
                " wildcard ('+'/'#')",
            )
        tokens = topic.split("/")
        for token in tokens:
            Uns.check_token(token, "topic token")
        if tokens[0] != Uns.ROOT:
            raise UnsValidationError(
                UnsValidationError.BAD_ROOT,
                f"topic '{topic}' must start with the UNS root '{Uns.ROOT}'"
                f" (got '{tokens[0]}')",
            )
        slashes = len(tokens) - 1
        if slashes > Uns.MAX_TOPIC_SLASHES:
            raise UnsValidationError(
                UnsValidationError.DEPTH_EXCEEDED,
                f"topic '{topic}' has {slashes} '/' separators (max {Uns.MAX_TOPIC_SLASHES})",
            )
        _check_length(topic)
        class_position = 5 if self._rooted(self._identity) else 4
        if len(tokens) <= class_position:
            raise UnsValidationError(
                UnsValidationError.BAD_CLASS,
                f"topic '{topic}' has too few levels ({len(tokens)}): the class token is"
                f" expected at position {class_position} (effective root mode"
                f" {self._rooted(self._identity)})",
            )
        cls = UnsClass.from_token(tokens[class_position])
        if cls is None:
            raise UnsValidationError(
                UnsValidationError.BAD_CLASS,
                f"'{tokens[class_position]}' (position {class_position} of '{topic}')"
                " is not a UNS class token",
            )
        has_channel = len(tokens) > class_position + 1
        if cls.leaf and has_channel:
            raise UnsValidationError(
                UnsValidationError.CHANNEL_ON_LEAF,
                f"class '{cls.token}' is a leaf class - topic '{topic}' must end at the"
                " class token",
            )
        if not cls.leaf and not has_channel:
            raise UnsValidationError(
                UnsValidationError.CHANNEL_REQUIRED,
                f"class '{cls.token}' requires at least one channel token - topic"
                f" '{topic}' ends at the class token",
            )

    @staticmethod
    def check_token(token: str, what: str) -> None:
        """The §2.2 **token rule** — deliberately the EXACT SAME blacklist as the config
        template sanitizer (``ConfigManager.sanitize``), so "sanitized => valid" is a
        true equivalence (D-U26): non-empty, no ``/ + # \\``, no ISO control characters
        (C0 U+0000–U+001F, U+007F, and C1 U+0080–U+009F), no ``..`` substring. Also the
        validation gate for ``gg.instance(id)`` instance tokens. If anyone later
        tightens the sanitizer, this rule must tighten with it (and vice versa).

        :raises UnsValidationError: ``EMPTY_TOKEN`` / ``BAD_CHAR`` / ``TRAVERSAL``
        """
        if not token:
            raise UnsValidationError(
                UnsValidationError.EMPTY_TOKEN, f"{what} must be a non-empty token"
            )
        for i, c in enumerate(token):
            o = ord(c)
            # D-U26: the sanitizer's control-char predicate (C0 U+0000-U+001F, U+007F
            # DEL, and C1 U+0080-U+009F).
            if c in "/+#\\" or o < 0x20 or 0x7F <= o <= 0x9F:
                raise UnsValidationError(
                    UnsValidationError.BAD_CHAR,
                    f"{what} '{token}' contains a forbidden character at index {i}"
                    " (no '/', '+', '#', '\\' or ISO control characters)",
                )
        if ".." in token:
            raise UnsValidationError(
                UnsValidationError.TRAVERSAL,
                f"{what} '{token}' contains the traversal sequence '..'",
            )

    def _rooted(self, target: MessageIdentity) -> bool:
        """The effective root mode for an identity (D-U25): ``topic.includeRoot``
        applies only when the identity carries a multi-level hierarchy — with a
        single-level hierarchy ``hier[0]`` *is* the device, so the site position does
        not exist and includeRoot is a no-op (``ConfigManager`` WARNs once at config
        time)."""
        return self._include_root and len(target.hier) >= 2


def _checked_token(token: str, what: str) -> str:
    """:meth:`Uns.check_token` that returns the (valid) token, for segment assembly."""
    Uns.check_token(token, what)
    return token


def _wildcard_or(value: Optional[str], what: str) -> str:
    """Renders a scope field: ``None`` as the ``+`` wildcard, else the checked token."""
    return "+" if value is None else _checked_token(value, what)


def _check_length(topic: str) -> None:
    """Enforces the 256-UTF-8-byte topic length limit."""
    nbytes = len(topic.encode("utf-8"))
    if nbytes > Uns.MAX_TOPIC_UTF8_BYTES:
        raise UnsValidationError(
            UnsValidationError.LENGTH_EXCEEDED,
            f"topic is {nbytes} UTF-8 bytes (max {Uns.MAX_TOPIC_UTF8_BYTES})",
        )
