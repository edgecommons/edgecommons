import argparse
import logging

import ggcommons
from app.greengrass_app import GreengrassApp

logger = logging.getLogger("main")


def main():
    arg_parser = argparse.ArgumentParser(
        description="Greengrass python component skeleton"
    )
    # add any component specific arguments here
    args, config_manager = ggcommons.init(
        component_name="PythonComponentSkeleton",
        arg_parser=arg_parser,
        receive_own_messages=True,
    )
    app = GreengrassApp(args=args, config_manager=config_manager)
    app.run()


if __name__ == "__main__":
    main()
