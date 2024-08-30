import argparse
import logging
from time import sleep
from typing import Tuple
from ggcommons.config.manager.config_manager import ConfigManager
from ggcommons.config.manager.config_manager_builder import ConfigManagerBuilder
from ggcommons.messaging.messaging_client import MessagingClient
from ggcommons.metrics.metric_emitter import MetricEmitter
from ggcommons.heartbeat.heartbeat import Heartbeat


def init(
    component_name: str, arg_parser: argparse.ArgumentParser, receive_own_messages=False
) -> Tuple[argparse.Namespace, ConfigManager, Heartbeat]:
    arg_parser.add_argument(
        "-c",
        "--config",
        nargs="*",
        type=str,
        default=["GG_CONFIG"],
        help="Configuration source.  One of: ENV, GG_CONFIG, FILE, SHADOW, CONFIG_COMPONENT (default: %(default)s)",
    )
    arg_parser.add_argument(
        "-m",
        "--messaging",
        nargs="*",
        type=str,
        default=["IPC"],
        help="Messaging provider. One of: IPC, MQTT (default: %(default)s)",
    )
    arg_parser.add_argument(
        "-t",
        "--thing",
        nargs="*",
        type=str,
        help="Thing name to use (optional)",
    )

    args = arg_parser.parse_args()

    logger = logging.getLogger("ggcommons")

    MessagingClient.init(args, receive_own_messages=receive_own_messages)
    logger.info("ggcommons: Messaging client initialized")

    config_manager = ConfigManagerBuilder.build(args, component_name)
    logger.info(
        f"ggcommons: Configuration loaded from {config_manager.get_config_source()}"
    )
    MetricEmitter.init(config_manager)
    logger.info("ggcommons: Metric Emitter initialized")
    heartbeat = Heartbeat(config_manager)
    logger.info("ggcommons: Heartbeat started")
    return args, config_manager, heartbeat

