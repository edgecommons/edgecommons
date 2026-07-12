"""Messaging error types for guarded requests and confirmed publication."""

from enum import Enum


class ReservedTopicError(ValueError):
    """Raised by the reserved-class publish guard (UNS-CANONICAL-DESIGN §4.1,
    D-U4/D-U8/D-U24) when a client-chosen topic targets a library-owned UNS class
    (``state | metric | cfg | log``). Components must not publish to reserved classes
    directly — the library publishers (heartbeat/state keepalive, the metric
    subsystem, the effective-config publisher) own those topics and reach them through
    the privileged ``MessagingClient._publish_reserved*`` seam.

    The guard is misuse prevention, not a security boundary — per-device broker ACLs
    are the durable enforcement (DESIGN-uns §7.5).
    """

    def __init__(self, topic: str, class_token: str):
        super().__init__(
            f"topic '{topic}' targets the reserved UNS class '{class_token}'"
            " (state|metric|cfg|log are library-owned): use the library publishers"
            " instead (heartbeat/state keepalive, the metric subsystem via"
            " gg.get_metrics(), the effective-config publisher)"
        )
        self.topic = topic
        self.class_token = class_token


class RequestTimeoutError(TimeoutError):
    """Raised by :meth:`edgecommons.utils.iou.Iou.get` when the framework-owned
    ``request()`` deadline fires before a reply arrives (UNS-CANONICAL-DESIGN §5,
    D-U5/D-U23). The deadline winner has already cleaned up the ephemeral reply
    subscription and removed the pending entry; a retry must issue a FRESH request."""


class PublishConfirmationReason(Enum):
    """Why a strict publish did not produce positive delivery evidence."""

    TIMEOUT = "TIMEOUT"
    TRANSPORT_ERROR = "TRANSPORT_ERROR"
    INTERRUPTED = "INTERRUPTED"


class PublishConfirmationError(RuntimeError):
    """A strict publish whose transport acknowledgement was not observed.

    The caller must treat delivery as unsuccessful or ambiguous and may retry the
    exact same serialized envelope.  In particular, a timeout is never success.
    """

    def __init__(
        self,
        reason: PublishConfirmationReason,
        message: str,
    ):
        if not isinstance(reason, PublishConfirmationReason):
            raise ValueError("reason must be a PublishConfirmationReason")
        super().__init__(message)
        self.reason = reason
