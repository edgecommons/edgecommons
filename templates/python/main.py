import argparse
import logging
import sys

from ggcommons import GGCommonsBuilder
from app.<<COMPONENTNAME>> import <<COMPONENTNAME>>

logger = logging.getLogger("main")


def main():
    arg_parser = argparse.ArgumentParser()
    # add any component specific arguments here

    # Construct the framework via the fluent builder. (The pre-rearch
    # ggcommons.init(...) entry point has been replaced by GGCommonsBuilder.)
    gg = (
        GGCommonsBuilder.create("<<COMPONENTNAME>>")
        .with_args(sys.argv[1:])
        .with_app_options(arg_parser)
        .receive_own_messages(True)
        .build()
    )
    # The app receives the framework facade: it mints its topics via gg.uns() (the UNS
    # topic builder bound to the config-resolved identity) and reaches config through it.
    app = <<COMPONENTNAME>>(gg)
    try:
        app.run()
    finally:
        gg.shutdown()


if __name__ == "__main__":
    logger.info("Starting <<COMPONENTNAME>>")
    main()
