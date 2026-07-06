"""The operator-event severity taxonomy (DESIGN-class-facades §2.2). The wire token is
the enum's **lowercase** value -- ``critical | warning | info | debug`` -- and it is
**the first channel token** of every ``evt`` publish:
:class:`~edgecommons.facades.events_facade.EventsFacade` derives the channel
``evt/{severity}/{type}`` from the body's own severity + type, so the topic and the body
can never disagree. A console subscribes ``ecv1/+/+/+/evt/critical/#`` for just alarms.

Mirrors Java's ``Severity`` enum (``com.mbreissi.edgecommons.facades.Severity``).
"""
from enum import Enum
from typing import Optional


class Severity(str, Enum):
    """The four-value operator-event severity. A ``str`` subclass so a ``Severity``
    member serializes as its own wire token when placed straight into a JSON-bound
    ``dict``."""

    #: An alarm-grade condition demanding operator attention (the ``raise_alarm`` default).
    CRITICAL = "critical"
    #: A degraded but non-critical condition.
    WARNING = "warning"
    #: An informational event (the ``emit`` default when the caller omits a severity).
    INFO = "info"
    #: A diagnostic event.
    DEBUG = "debug"

    def wire(self) -> str:
        """The wire token -- the lowercase value, the ``evt`` channel's first token."""
        return self.value

    @staticmethod
    def from_wire(token: Optional[str]) -> Optional["Severity"]:
        """Resolves a lowercase wire token to its severity, or ``None`` when outside the
        closed set."""
        try:
            return Severity(token)
        except ValueError:
            return None
