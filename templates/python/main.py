import argparse
import logging
import sys

from edgecommons import EdgeCommonsBuilder
from app.<<COMPONENTNAME>> import <<COMPONENTNAME>>, GreetingState, SET_GREETING

logger = logging.getLogger("main")


def main():
    arg_parser = argparse.ArgumentParser()
    # add any component specific arguments here
    command_state = GreetingState()

    # Construct the framework via the fluent builder. (The pre-rearch
    # edgecommons.init(...) entry point has been replaced by EdgeCommonsBuilder.)
    gg = (
        EdgeCommonsBuilder.create("<<COMPONENTNAME>>")
        .with_args(sys.argv[1:])
        .with_app_options(arg_parser)
        .receive_own_messages(True)
        .initial_ready(False)
        # Install component handlers before MQTT SUBACK / Greengrass subscription
        # acknowledgement can make the command inbox ACTIVE.
        .configure_commands(
            lambda inbox: inbox.register(SET_GREETING, command_state.handle)
        )
        .build()
    )
    # The app receives the framework facade: it mints its topics via gg.uns() (the UNS
    # topic builder bound to the config-resolved identity) and reaches config through it.
    app = <<COMPONENTNAME>>(gg, command_state)
    # App construction completed; messaging and acknowledged command activation remain
    # mandatory parts of the library-owned readiness predicate.
    gg.set_ready(True)
    try:
        app.run()
    finally:
        gg.shutdown()


if __name__ == "__main__":
    logger.info("Starting <<COMPONENTNAME>>")
    main()
