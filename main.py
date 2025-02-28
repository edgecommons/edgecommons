import argparse
import logging

import ggcommons
from app.<<COMPONENT_NAME>> import <<COMPONENT_NAME>>

logger = logging.getLogger("main")


def main():
    arg_parser = argparse.ArgumentParser()
    # add any component specific arguments here
    args, config_manager, heartbeat = ggcommons.init(
        component_name="<<COMPONENT_NAME>>",
        arg_parser=arg_parser,
        receive_own_messages=True,
    )
    app = <<COMPONENT_NAME>>(args=args, config_manager=config_manager)
    app.run()


if __name__ == "__main__":
    logger.info("Starting <<COMPONENT_NAME>>")
    main()
