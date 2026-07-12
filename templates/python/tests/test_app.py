"""The scaffold's two seams: the command verb it registers, and the connectivity it reports.

Neither needs a broker, a transport or a device. The app is handed the framework facade, so a
recording stand-in for it is enough to assert what the app wires into the library — which is what
you will do with your own verbs and your own provider. Run them with `pytest`.
"""
import pytest

from app.<<COMPONENTNAME>> import SET_GREETING, <<COMPONENTNAME>>


# --- the framework stand-in: it records what the app registers ----------------------------------


class FakeCommands:
    def __init__(self):
        self.verbs = {}

    def register(self, verb, handler):
        self.verbs[verb] = handler


class FakeGg:
    """Records the app's registrations; every getter returns something inert."""

    def __init__(self):
        self.commands = FakeCommands()
        self.connectivity_provider = None

    def get_config_manager(self):
        return self

    def add_config_change_listener(self, listener):
        pass

    def get_messaging(self):
        return self

    def get_metrics(self):
        return self

    def define_metric(self, metric):
        pass

    def get_commands(self):
        return self.commands

    def data(self):
        return self

    def events(self):
        return self

    def set_instance_connectivity_provider(self, provider):
        self.connectivity_provider = provider


class FakeRequest:
    def __init__(self, body):
        self._body = body

    def get_body(self):
        return self._body


@pytest.fixture
def gg():
    return FakeGg()


@pytest.fixture
def app(gg):
    return <<COMPONENTNAME>>(gg)


# --- instance connectivity -----------------------------------------------------------------------


def test_the_component_registers_an_instance_connectivity_provider(app, gg):
    # ONE provider, TWO surfaces: the library pushes this sample into every `state` keepalive's
    # instances[] AND returns it from the built-in `status` verb. A console that subscribes and a
    # console that asks cannot get different answers.
    assert gg.connectivity_provider is not None


def test_a_component_that_owns_no_connections_reports_no_instances(app):
    # No instances -> no instances[] section -> `status` answers exactly as `ping`. That is a real
    # answer, not a missing one. Replace this with one entry per connection once the component
    # owns any, and assert here that a configured-but-down connection is still reported.
    assert app.instance_connectivity() == []


# --- the custom command verb ---------------------------------------------------------------------


def test_the_custom_verb_is_registered_alongside_the_built_ins(app, gg):
    assert SET_GREETING in gg.commands.verbs


def test_set_greeting_replaces_the_greeting_and_reports_what_it_replaced(app, gg):
    handler = gg.commands.verbs[SET_GREETING]

    reply = handler(FakeRequest({"greeting": "Hi"}))

    assert reply["greeting"] == "Hi"
    assert reply["previousGreeting"] != "Hi"


def test_a_malformed_command_argument_is_a_coded_error_not_a_crash(app, gg):
    from edgecommons.command_inbox import CommandException

    handler = gg.commands.verbs[SET_GREETING]

    with pytest.raises(CommandException):
        handler(FakeRequest({"greetnig": "typo"}))
