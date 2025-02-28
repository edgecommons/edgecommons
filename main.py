import argparse
import logging

import ggcommons
from app.<<COMPONENTNAME>> import <<COMPONENTNAME>>

logger = logging.getLogger("main")


def main():
    arg_parser = argparse.ArgumentParser()
    # add any component specific arguments here
    args, config_manager, heartbeat = ggcommons.init(
        component_name="<<COMPONENTNAME>>",
        arg_parser=arg_parser,
        receive_own_messages=True,
    )
    app = <<COMPONENTNAME>>(args=args, config_manager=config_manager)
    app.run()


if __name__ == "__main__":
    logger.info("Starting <<COMPONENTNAME>>")
    main()
