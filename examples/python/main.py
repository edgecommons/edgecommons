import argparse
import logging
import sys

from ggcommons import GGCommonsBuilder
from app.greengrass_app import GreengrassApp

logger = logging.getLogger("main")


def main():
    arg_parser = argparse.ArgumentParser(
        description="Greengrass python component skeleton"
    )
    # add any component specific arguments here

    # Construct the framework via the fluent builder. (The pre-rearch
    # ggcommons.init(...) entry point has been replaced by GGCommonsBuilder.)
    gg = (
        GGCommonsBuilder.create("PythonComponentSkeleton")
        .with_args(sys.argv[1:])
        .with_app_options(arg_parser)
        .receive_own_messages(True)
        .build()
    )
    config_manager = gg.get_config_manager()
    app = GreengrassApp(config_manager=config_manager)
    try:
        app.run()
    finally:
        gg.shutdown()


if __name__ == "__main__":
    logger.info("Staring Python Component Skeleton")
    main()
