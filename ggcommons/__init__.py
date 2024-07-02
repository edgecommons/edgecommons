import argparse
import logging
from time import sleep
from typing import Tuple
from ggcommons.config.manager.config_manager import ConfigManager
from ggcommons.config.manager.config_manager_builder import ConfigManagerBuilder
from ggcommons.heartbeat.heartbeat import Heartbeat
from ggcommons.messaging.messaging_client import MessagingClient
from ggcommons.metrics.metric_emitter import MetricEmitter


def init(
    component_name: str, arg_parser: argparse.ArgumentParser, receive_own_messages=False
) -> Tuple[argparse.Namespace, ConfigManager]:
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
    Heartbeat(config_manager)
    logger.info("ggcommons: Heartbeat started")
    return args, config_manager


if __name__ == "__main__":
    import sys
    from ggcommons.metrics.metric import Metric
    from ggcommons.metrics.measure import Measure
    from random import random

    sys.argv = [
        "ggcommons_python",
        "--config", "FILE", "../config_3.json",
        "--messaging", "MQTT", "a3bgkcole5zuv-ats.iot.us-east-1.amazonaws.com", "443", "../creds",
        "--thing", "ggcommons-test-2"
    ]
    init("ggcommons_python", argparse.ArgumentParser())
    metric = Metric(name="performance")
    metric.add_measure(Measure("latency", "Milliseconds", 1))
    MetricEmitter.define_metric(metric)

    while True:
        measure_values = {"replyLatency": random() * 100}
        MetricEmitter.emit_metric("performance", measure_values)
        sleep(1)
