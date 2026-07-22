"""<<COMPONENTNAME>> entry point — a southbound protocol-adapter on edgecommons.

Builds the framework, then hands off to :class:`<<SNAKENAME>>.adapter.App`, which spawns one worker
thread per ``component.instances[]`` entry (each device connects/retries independently), registers
the ``sb/*`` command surface + the instance-connectivity provider, and blocks until shutdown. The
library owns SIGTERM/SIGINT -> graceful shutdown.
"""
import argparse
import logging
import sys

from edgecommons import EdgeCommonsBuilder

from <<SNAKENAME>>.adapter import App

logger = logging.getLogger("main")


def main():
    arg_parser = argparse.ArgumentParser(description="<<COMPONENTNAME>> southbound adapter")
    gg = (
        EdgeCommonsBuilder.create("<<COMPONENTFULLNAME>>")
        .with_args(sys.argv[1:])
        .with_app_options(arg_parser)
        .initial_ready(False)
        .build()
    )
    App(gg).run()


if __name__ == "__main__":
    main()
