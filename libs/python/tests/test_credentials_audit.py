"""Credential access-audit tests: events emitted on get/put/delete; value never leaked."""
from dataclasses import asdict

from ggcommons.credentials import (
    AuditEvent,
    AuditSink,
    DefaultCredentialService,
    FileKeyProvider,
    LocalVault,
    open_from_config,
)


class CollectingSink(AuditSink):
    """Test sink that records every AuditEvent for assertion."""

    def __init__(self):
        self.events = []

    def record(self, event: AuditEvent) -> None:
        self.events.append(event)


SECRET_VALUE = "super-secret-do-not-log"


def _svc(tmp_path, sink) -> DefaultCredentialService:
    provider = FileKeyProvider(bytes([7] * 32))
    return DefaultCredentialService(
        LocalVault.open(str(tmp_path / "vault"), provider, 2), audit=sink
    )


def test_audit_emits_put_get_hit_get_miss_delete(tmp_path):
    sink = CollectingSink()
    c = _svc(tmp_path, sink)

    version = c.put("db/password", SECRET_VALUE.encode())   # put -> ok
    assert c.get_string("db/password") == SECRET_VALUE       # get -> hit
    assert c.get("missing") is None                          # get -> miss
    assert c.delete("db/password") is True                   # delete -> ok

    tuples = [(e.op, e.name, e.version, e.source, e.outcome) for e in sink.events]
    assert tuples == [
        ("put", "db/password", version, "local", "ok"),
        ("get", "db/password", version, "local", "hit"),
        ("get", "missing", "-", "-", "miss"),
        ("delete", "db/password", "-", "-", "ok"),
    ]


def test_audit_delete_miss(tmp_path):
    sink = CollectingSink()
    c = _svc(tmp_path, sink)
    assert c.delete("never/existed") is False
    assert (sink.events[-1].op, sink.events[-1].outcome) == ("delete", "miss")


def test_audit_get_version_emits_get_event(tmp_path):
    sink = CollectingSink()
    c = _svc(tmp_path, sink)
    version = c.put("k", b"v1")
    sink.events.clear()

    c.get_version("k", version)          # hit
    c.get_version("k", "doesnotexist")   # miss

    tuples = [(e.op, e.name, e.version, e.outcome) for e in sink.events]
    assert tuples == [
        ("get", "k", version, "hit"),
        ("get", "k", "doesnotexist", "miss"),
    ]


def test_audit_never_contains_secret_value(tmp_path):
    sink = CollectingSink()
    c = _svc(tmp_path, sink)
    c.put("db/password", SECRET_VALUE.encode())
    c.get("db/password")
    c.delete("db/password")

    for e in sink.events:
        for v in asdict(e).values():
            assert SECRET_VALUE not in str(v)


def test_no_sink_is_noop(tmp_path):
    """A service with no audit sink (default) must not error on any op."""
    provider = FileKeyProvider(bytes([7] * 32))
    c = DefaultCredentialService(LocalVault.open(str(tmp_path / "vault"), provider, 2))
    v = c.put("k", b"v1")
    assert c.get_string("k") == "v1"
    assert c.get("missing") is None
    assert c.get_version("k", v).as_str() == "v1"
    assert c.delete("k") is True
    assert c.delete("k") is False  # delete miss, still no error


def test_with_audit_setter(tmp_path):
    """with_audit() attaches the sink fluently after construction."""
    sink = CollectingSink()
    provider = FileKeyProvider(bytes([7] * 32))
    c = DefaultCredentialService(LocalVault.open(str(tmp_path / "vault"), provider, 2)).with_audit(sink)
    c.put("k", b"v")
    assert sink.events and sink.events[0].op == "put"


def test_config_audit_enabled_default(tmp_path):
    """open_from_config wires the default log sink unless audit.enabled is false."""
    cfg = {"vault": {"path": str(tmp_path / "vault")}}
    c = open_from_config(cfg)
    assert c._audit is not None  # default on


def test_config_audit_disabled(tmp_path):
    cfg = {"vault": {"path": str(tmp_path / "vault2")}, "audit": {"enabled": False}}
    c = open_from_config(cfg)
    assert c._audit is None
