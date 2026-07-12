"""What this adapter reports about its devices — the seam a console reads.

`LinkStatus` is pure logic over the configured devices, so the invariant that matters most here is
testable with no protocol client, no broker and no transport: a device that is configured but not
reachable is *reported*, and says which kind of unreachable it is. Run them with `pytest`.
"""
from app.<<COMPONENTNAME>> import BACKOFF, CONNECTING, ONLINE, LinkStatus, link_statuses


class FakeConfigManager:
    """The two calls `link_statuses` makes."""

    def __init__(self, instances):
        self._instances = instances

    def get_instance_ids(self):
        return list(self._instances)

    def get_instance_config(self, instance_id):
        return self._instances[instance_id]


def test_a_configured_but_not_yet_connected_device_is_still_reported():
    # THE invariant. A device that is configured and down must never be indistinguishable from one
    # that was never configured — so it is reported from the first keepalive, before its worker has
    # connected anything: not reachable, and CONNECTING says why it is not reachable *yet*.
    link = LinkStatus("plc-1", {"adapter": "modbus", "connection": {"endpoint": "tcp://plc-1:502"}})

    c = link.connectivity()

    assert c.instance == "plc-1"
    assert c.connected is False, "the normalized flag every console reads"
    assert c.state == CONNECTING, "not BACKOFF: nothing has failed yet"
    assert c.detail == "tcp://plc-1:502", "the endpoint, for a human"
    assert c.attributes == {"adapter": "modbus"}, "the open bag carries this adapter's domain data"


def test_a_device_reports_online_once_its_link_is_up():
    link = LinkStatus("plc-1", {"connection": {"endpoint": "tcp://plc-1:502"}})

    link.set(ONLINE)

    assert link.connectivity().connected is True
    assert link.connectivity().state == ONLINE


def test_a_failed_link_is_backoff_and_carries_the_reason():
    # Both CONNECTING and BACKOFF are connected=false, and they are not the same thing: one has
    # never been up, the other just fell over and is retrying. The boolean alone cannot tell them
    # apart; the state token can.
    link = LinkStatus("plc-1", {"connection": {"endpoint": "tcp://plc-1:502"}})

    link.set(BACKOFF, "connection refused")

    c = link.connectivity()
    assert c.connected is False
    assert c.state == BACKOFF
    assert c.detail == "connection refused"


def test_every_configured_device_gets_a_link_status():
    cm = FakeConfigManager({"plc-1": {"connection": {}}, "plc-2": {"connection": {}}})

    links = link_statuses(cm)

    assert sorted(links) == ["plc-1", "plc-2"]
    # The provider main.py registers reports one entry per configured device, always.
    reported = [link.connectivity() for link in links.values()]
    assert [c.instance for c in reported] == ["plc-1", "plc-2"]
    assert all(c.connected is False for c in reported)
