import argparse
import logging

from ggcommons.config.manager.config_manager import ConfigManager
from ggcommons.config.manager.config_manager_builder import ConfigManagerBuilder
from ggcommons.heartbeat.heartbeat import Heartbeat
from ggcommons.messaging.messaging_client import MessagingClient


def init(component_name: str, arg_parser: argparse.ArgumentParser, receive_own_messages=False)\
        -> tuple[argparse.Namespace, ConfigManager]:
    arg_parser.add_argument("-c", "--config", nargs="*", type=str, default=["GG_CONFIG"],
                        help="Configuration source.  One of: ENV, GG_CONFIG, FILE, SHADOW (default: %(default)s)")
    arg_parser.add_argument("-m", "--messaging", nargs="*", type=str, default=["IPC"],
                        help="Messaging provider. One of: IPC, MQTT (default: %(default)s)")
    args = arg_parser.parse_args()

    logger = logging.getLogger("ggcommons")

    MessagingClient.init(args.messaging, receive_own_messages=receive_own_messages)
    logger.info("ggcommons: Messaging client initialized")

    config_manager = ConfigManagerBuilder.build(args.config, component_name)
    logger.info(f"ggcommons: Configuration loaded from {config_manager.get_config_source()}")

    Heartbeat(config_manager)
    logger.info("ggcommons: Heartbeat started")
    return args, config_manager
