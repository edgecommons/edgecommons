from edgecommons.messaging.message_builder import MessageBuilder
from edgecommons.messaging.messaging_client import MessagingClient
from edgecommons.messaging.qos import Qos
from edgecommons.config.manager.config_manager import ConfigManager
from edgecommons.metrics.targets.emf_helper import build_metric_data_emf
from edgecommons.metrics.targets.metric_target import MetricTarget
from edgecommons.uns import Uns, UnsClass


def _is_local_destination(destination: str) -> bool:
    """True for the local/IPC transport, False for northbound.

    Northbound is selected only by "northbound"; everything else ("ipc", "local", and any
    unrecognized value) uses the local transport, so a metric never fails by routing to an
    unconfigured northbound broker. Matches the Java/Rust metric targets and the config schema.
    """
    return destination is None or destination.lower() != "northbound"


class Messaging(MetricTarget):
    """The ``messaging`` metric target (UNS-CANONICAL-DESIGN §4.3): publishes each
    metric to the library-owned UNS metric topic
    ``ecv1/{device}/{component}/main/metric/{metricName}`` (the metric name sanitized
    as a channel token) through the privileged ``MessagingClient._publish_reserved*``
    seam — the ``metric`` class is reserved. ``metricEmission.targetConfig.destination``
    still selects local/IPC vs northbound (D-U9); the legacy ``targetConfig.topic``
    override is removed."""

    def __init__(self, config_manager: ConfigManager):
        super().__init__(config_manager)
        self.send_to_local = _is_local_destination(
            config_manager.get_metric_config().get_destination()
        )
        # WARN-once flag for the no-resolved-identity (test/subclass bring-up) case.
        self._warned_no_identity = False

    def emit_metric_now(self, metric, measure_values):
        metric_name = metric.get_name()
        self.logger.debug(f"Emitting metric '{metric_name}' to messaging target with {len(measure_values)} measures")

        topic = self._metric_topic(metric_name)
        if topic is None:
            return

        metric_data = build_metric_data_emf(
            self.metric_config, metric, measure_values, False
        )
        self.__publish_message(topic, metric_data)

        if self.metric_config.get_large_fleet_workaround():
            self.logger.debug(f"Emitting large fleet workaround metric for '{metric_name}'")
            metric_data = build_metric_data_emf(
                self.metric_config, metric, measure_values, True
            )
            self.__publish_message(topic, metric_data)

        self.logger.debug(f"Metric '{metric_name}' emission completed")

    def _metric_topic(self, metric_name: str):
        """The metric's UNS topic —
        ``ecv1[/{site}]/{device}/{component}/metric/{name}`` (component scope, no
        instance — D-U28) with the metric name passed through the template sanitizer
        (the §2.2 channel-token rule), or
        ``None`` (WARN once) when no component identity is resolved (mock/test
        bring-up)."""
        identity = self.config_manager.get_component_identity()
        if identity is None:
            if not self._warned_no_identity:
                self._warned_no_identity = True
                self.logger.warning(
                    "No resolved component identity - the messaging metric target"
                    " cannot build UNS metric topics; metrics are dropped"
                )
            return None
        return Uns(identity, self.config_manager.is_topic_include_root()).topic(
            UnsClass.METRIC, ConfigManager.sanitize(metric_name)
        )

    def __publish_message(self, topic: str, metric_dict: dict):
        destination = "local" if self.send_to_local else "northbound"
        self.logger.debug(f"Publishing metric message to {destination} on topic: {topic}")

        message = MessageBuilder.create("Metric", "1.0") \
            .with_payload(metric_dict) \
            .with_config(self.config_manager) \
            .build()

        # The metric class is reserved (§4.1) - publish through the privileged seam
        # (§4.2).
        if self.send_to_local:
            MessagingClient._publish_reserved(topic, message)
        else:
            MessagingClient._publish_reserved_northbound(topic, message, Qos.AT_LEAST_ONCE)

    def on_configuration_change(self, configuration) -> bool:
        self.logger.info("Metric messaging configuration changed, reconfiguring target")

        old_destination = "local" if self.send_to_local else "northbound"
        self.send_to_local = _is_local_destination(
            self.config_manager.get_metric_config().get_destination()
        )
        new_destination = "local" if self.send_to_local else "northbound"

        self.logger.info(f"Metric messaging reconfigured - destination: {old_destination} -> {new_destination}")
        return True
