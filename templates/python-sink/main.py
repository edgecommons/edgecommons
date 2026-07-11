"""<<COMPONENTNAME>> entry point -- a sink component on edgecommons.

Builds the framework, then hands control to the app: one sink per ``component.instances[]`` entry,
each with its own bounded queue, destination and worker thread. The library owns SIGTERM/SIGINT ->
graceful shutdown.

Run locally (HOST platform, MQTT transport, against a local MQTT broker):

.. code-block:: bash

    python3 main.py --platform HOST --transport MQTT ./test-configs/standalone-messaging.json \
      -c FILE ./test-configs/config.json -t my-thing
"""
import argparse
import logging
import sys

from edgecommons import EdgeCommonsBuilder

from app.<<COMPONENTNAME>> import <<COMPONENTNAME>>

logger = logging.getLogger("main")


def main():
    arg_parser = argparse.ArgumentParser(description="<<COMPONENTFULLNAME>> -- a sink component")
    # add any component specific arguments here

    gg = (
        EdgeCommonsBuilder.create("<<COMPONENTFULLNAME>>")
        .with_args(sys.argv[1:])
        .with_app_options(arg_parser)
        # A sink delivers other components' messages outward; it has no reason to consume its own.
        .receive_own_messages(False)
        .build()
    )
    # Not ready until the sinks are subscribed and running (the app flips this in run()).
    gg.set_ready(False)

    app = <<COMPONENTNAME>>(gg)
    try:
        app.run()
    finally:
        app.stop()
        gg.shutdown()


if __name__ == "__main__":
    main()
