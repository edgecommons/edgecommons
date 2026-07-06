"""Credential access audit — emit non-sensitive access events to a pluggable sink.

A secrets subsystem should record who-touched-what-when. :class:`DefaultCredentialService`
emits an :class:`AuditEvent` on each value-touching / mutating op (``get`` / ``get_version`` /
``put`` / ``delete``) when an audit sink is configured (``credentials.audit.enabled``). The
default :class:`LogAuditSink` writes a structured record on a dedicated logger so the audit trail
can be filtered / routed independently; a custom :class:`AuditSink` can forward events to any
log / metric / SIEM pipeline.

Events carry only metadata — the secret value is **never** included.
"""
import logging
from abc import ABC, abstractmethod
from dataclasses import dataclass

# The logger the default sink emits on (filter / route the audit trail independently).
AUDIT_LOGGER = "edgecommons.credentials.audit"


@dataclass
class AuditEvent:
    """A single credential-access audit event. **Never contains the secret value.**"""

    # Operation: "get" | "put" | "delete".
    op: str
    # Caller-facing secret name (transparent namespace stripped).
    name: str
    # Version touched, or "-" when not applicable / not found.
    version: str
    # Origin of the value: "local" | "central" | "-".
    source: str
    # Result: "hit" | "miss" | "ok".
    outcome: str


class AuditSink(ABC):
    """Destination for audit events. Must be cheap, non-blocking, and never log the value."""

    @abstractmethod
    def record(self, event: AuditEvent) -> None:
        """Record one access event."""
        raise NotImplementedError


class LogAuditSink(AuditSink):
    """Default sink: emit each event as a structured log record on :data:`AUDIT_LOGGER`."""

    def __init__(self):
        self._logger = logging.getLogger(AUDIT_LOGGER)

    def record(self, event: AuditEvent) -> None:
        self._logger.info(
            "credential access op=%s secret=%s version=%s source=%s outcome=%s",
            event.op,
            event.name,
            event.version,
            event.source,
            event.outcome,
        )


def log_sink() -> AuditSink:
    """The default logging audit sink (used when ``credentials.audit.enabled``)."""
    return LogAuditSink()
