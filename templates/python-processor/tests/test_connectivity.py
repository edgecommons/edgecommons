"""What this processor reports about its instances — the seam a console reads.

`test_pipeline.py` covers the payload-agnostic core; this covers the one thing the app wires into
the library. The app is handed the framework facade, so a recording stand-in for it is enough. Run
them with `pytest`.
"""
from app.<<COMPONENTNAME>> import <<COMPONENTNAME>>


class FakeIdentity:
    path = "factory-1/gw-01"
    component = "my-processor"


class FakeGg:
    """Records the app's registrations; every getter returns something inert."""

    ROUTE = {"id": "temps", "subscribe": ["ecv1/+/+/+/data/#"], "publishTopic": "t"}

    def __init__(self):
        self.connectivity_provider = None

    def get_config_manager(self):
        return self

    def add_config_change_listener(self, listener):
        pass

    def get_global_config(self):
        return {}

    def get_instance_ids(self):
        return ["temps"]

    def get_instance_config(self, instance_id):
        return self.ROUTE

    def get_component_identity(self):
        return FakeIdentity()

    def get_messaging(self):
        return self

    def get_metrics(self):
        return self

    def define_metric(self, metric):
        pass

    def set_instance_connectivity_provider(self, provider):
        self.connectivity_provider = provider


def test_the_component_registers_an_instance_connectivity_provider():
    # ONE provider, TWO surfaces: the library pushes this sample into every `state` keepalive's
    # instances[] AND returns it from the built-in `status` verb. A console that subscribes and a
    # console that asks cannot get different answers.
    gg = FakeGg()

    <<COMPONENTNAME>>(gg)

    assert gg.connectivity_provider is not None


def test_a_processor_owns_no_connections_so_it_reports_no_instances():
    # A route is a subscription, not a link to a device. No instances -> no instances[] section ->
    # `status` answers exactly as `ping`. That is a real answer, not a missing one. Once a stage of
    # yours does own a connection, report it here — and assert that a configured-but-down one is
    # still reported.
    gg = FakeGg()

    app = <<COMPONENTNAME>>(gg)

    assert app.instance_connectivity() == []
    assert gg.connectivity_provider() == []
