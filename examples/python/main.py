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
    # Use the FULL component name (matches the recipe ComponentName + the Rust/Java/TS skeletons),
    # so {ComponentFullName} in config templates resolves to the Greengrass-managed, ggc_user-owned
    # work dir (/greengrass/v2/work/<full-name>). A short name resolves to a path under the
    # root-owned /greengrass/v2/work that the component can't create (vault/stream dirs then fail).
    gg = (
        GGCommonsBuilder.create("aws.proserve.greengrass.PythonComponentSkeleton")
        .with_args(sys.argv[1:])
        .with_app_options(arg_parser)
        .receive_own_messages(True)
        .build()
    )
    config_manager = gg.get_config_manager()
    # Telemetry streaming service (None unless the config has a `streaming` section); the app
    # appends durable records to it and the library's export engine drains them to the sink.
    streams = gg.get_streams()
    # Demonstrate encrypted-vault secret access once at startup (non-fatal).
    GreengrassApp.demonstrate_credentials(gg)
    # Demonstrate externalized-parameter access once at startup (non-fatal).
    GreengrassApp.demonstrate_parameters(gg)
    app = GreengrassApp(config_manager=config_manager, streams=streams)
    try:
        app.run()
    finally:
        gg.shutdown()


if __name__ == "__main__":
    logger.info("Staring Python Component Skeleton")
    main()
