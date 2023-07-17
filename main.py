import argparse
import asyncio
import logging

import ggcommons
from app.greengrass_app import GreengrassApp

logger = logging.getLogger("main")


async def main():
    arg_parser = argparse.ArgumentParser(description="Greengrass python component skeleton")
    # add any component specific arguments here
    args, config_manager = ggcommons.init(component_name="TestSkeletonComponent", arg_parser=arg_parser,
                                          receive_own_messages=True)
    app = GreengrassApp(args=args, config_manager=config_manager)
    await app.run()


if __name__ == "__main__":
    loop = asyncio.get_event_loop()
    loop.run_until_complete(main())
