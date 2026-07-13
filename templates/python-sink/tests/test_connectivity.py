"""What this sink reports about its destinations — the seam a console reads.

`test_dest.py` covers the pure destination/retry/health logic; this covers the one thing the app
wires into the library. The app is handed the framework facade, so a recording stand-in for it is
enough. Run them with `pytest`.
"""
from app.dest import BACKOFF, CONNECTING, FAILED, ONLINE
from app.<<COMPONENTNAME>> import <<COMPONENTNAME>>


class FakeGg:
    """Records the app's registrations; every getter returns something inert."""

    SINKS = {
        "archive": {
            "id": "archive",
            "subscribe": "ecv1/+/+/+/data/#",
            "destination": {"type": "local", "path": "/var/lib/out"},
        },
        "audit": {
            "id": "audit",
            "subscribe": "ecv1/+/+/+/evt/#",
            "destination": {"type": "local", "path": "/var/lib/audit"},
        },
    }

    def __init__(self):
        self.connectivity_provider = None

    def get_config_manager(self):
        return self

    def add_config_change_listener(self, listener):
        pass

    def get_global_config(self):
        return {}

    def get_instance_ids(self):
        return list(self.SINKS)

    def get_instance_config(self, instance_id):
        return self.SINKS[instance_id]

    def get_messaging(self):
        return self

    def get_metrics(self):
        return self

    def define_metric(self, metric):
        pass

    def set_instance_connectivity_provider(self, provider):
        self.connectivity_provider = provider


def test_every_configured_destination_is_reported_before_anything_is_delivered():
    # A sink's destinations ARE its instances, and one that is configured but not delivering must
    # never be indistinguishable from one that was never configured. ONE provider, TWO surfaces: the
    # library pushes this into every `state` keepalive's instances[] AND returns it from the
    # built-in `status` verb, so a console that subscribes and one that asks cannot disagree.
    gg = FakeGg()

    app = <<COMPONENTNAME>>(gg)

    assert gg.connectivity_provider is not None
    reported = app.instance_connectivity()
    assert [c.instance for c in reported] == ["archive", "audit"]
    assert all(c.connected is False for c in reported), "nothing has been verified yet"
    assert all(c.state == CONNECTING for c in reported)
    assert reported[0].attributes == {"destination": "local"}


def test_a_destinations_condition_reaches_the_wire_element():
    # ONLINE only after a delivery is verified; BACKOFF and FAILED are both connected=false and stay
    # tellable apart -- still trying is not the same as gave up.
    app = <<COMPONENTNAME>>(FakeGg())

    app.health["archive"].delivered("archive/temp/uuid-1.json")
    app.health["audit"].retrying("transient: connection reset")
    archive, audit = app.instance_connectivity()

    assert (archive.connected, archive.state) == (True, ONLINE)
    assert (audit.connected, audit.state) == (False, BACKOFF)

    app.health["audit"].failed("permanent: bad credentials")
    assert app.instance_connectivity()[1].state == FAILED
