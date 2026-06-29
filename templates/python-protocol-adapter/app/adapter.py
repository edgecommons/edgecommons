"""<<COMPONENTNAME>> — southbound protocol-adapter skeleton (docs/SOUTHBOUND.md).

Replace the TODOs with your protocol client. One instance of this class runs per
``component.instances[]`` entry. It should connect to its source (retrying until up), then either
subscribe or poll it and republish value changes as ``SouthboundTagUpdate`` messages, and serve the
optional read/write/control command surface. See the OPC UA (subscribe) and Modbus (poll) reference
adapters for full implementations.
"""
import logging
import threading
from datetime import datetime, timezone

from ggcommons.messaging.message_builder import MessageBuilder
from ggcommons.messaging.messaging_client import MessagingClient
from ggcommons.metrics.metric_builder import MetricBuilder
from ggcommons.metrics.metric_emitter import MetricEmitter

logger = logging.getLogger("<<COMPONENTNAME>>")

ADAPTER = "example"   # TODO: your protocol id, e.g. "modbus" / "opcua"


def _now_iso():
    return datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")


class <<COMPONENTNAME>>:
    def __init__(self, config_manager, instance_id):
        self._cm = config_manager
        self._id = instance_id
        self._inst = config_manager.get_instance_config(instance_id) or {}
        self._stop = threading.Event()

        # Standard southbound health metric (contract §5).
        MetricEmitter.define_metric(
            MetricBuilder.create("southbound_health").with_config(config_manager)
            .add_measure("connectionState", "Count", 1)
            .add_measure("readErrors", "Count", 60)
            .add_dimension("instance", instance_id)
            .build()
        )
        # TODO: construct + connect your protocol client here (block/retry until connected).
        logger.info("[%s] starting", instance_id)

    def run(self):
        """Subscribe or poll the source and publish changes. (Poll loop shown.)"""
        interval = self._inst.get("pollIntervalMs", 1000) / 1000.0
        while not self._stop.wait(interval):
            try:
                value = self._read_one()                      # TODO: read from your device
                self._publish("ExampleTag", value)
                MetricEmitter.emit_metric("southbound_health", {"connectionState": 1.0, "readErrors": 0.0})
            except Exception as e:  # noqa: BLE001
                logger.error("[%s] poll failed: %s", self._id, e)
                MetricEmitter.emit_metric("southbound_health", {"connectionState": 0.0, "readErrors": 1.0})

    def _read_one(self):
        return 0  # TODO: read a real value from your protocol client

    def _publish(self, tag_name, value):
        body = {
            "device": {"adapter": ADAPTER, "instance": self._id, "endpoint": "TODO"},
            "tag": {"id": f"{self._id}/{tag_name}", "name": tag_name, "address": {"tag": tag_name}},
            "samples": [{"value": value, "quality": "GOOD", "qualityRaw": "Good",
                         "sourceTs": None, "serverTs": _now_iso()}],
        }
        topic = self._cm.resolve_template(
            f"southbound/{{ComponentName}}/{self._id}/{tag_name}")
        msg = MessageBuilder.create("SouthboundTagUpdate", "1.0").with_payload(body).with_config(self._cm).build()
        MessagingClient.publish(topic, msg)

    def stop(self):
        self._stop.set()
        # TODO: close your protocol client.
