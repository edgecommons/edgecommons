"""<<COMPONENTNAME>> — southbound protocol-adapter skeleton (docs/SOUTHBOUND.md).

Replace the TODOs with your protocol client. One instance of this class runs per
``component.instances[]`` entry. It should connect to its source (retrying until up), then either
subscribe or poll it and republish value changes as ``SouthboundSignalUpdate`` messages on this
instance's UNS ``data`` topic (``ecv1/{device}/{component}/{instance}/data/{signalPath}``), and
serve the optional read/write/control command surface. See the OPC UA (subscribe) and Modbus
(poll) reference adapters for full implementations.
"""
import logging
import threading
from datetime import datetime, timezone

from ggcommons.config.manager.config_manager import ConfigManager
from ggcommons.metrics.metric_builder import MetricBuilder
from ggcommons.metrics.metric_emitter import MetricEmitter
from ggcommons.uns import UnsClass

logger = logging.getLogger("<<COMPONENTNAME>>")

ADAPTER = "example"   # TODO: your protocol id, e.g. "modbus" / "opcua"


def _now_iso():
    return datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")


class <<COMPONENTNAME>>:
    def __init__(self, gg, instance_id):
        self._gg = gg
        self._cm = gg.get_config_manager()
        self._id = instance_id
        self._inst = self._cm.get_instance_config(instance_id) or {}
        self._stop = threading.Event()
        # The instance-scoped handle (gg.instance(id)): its uns() mints this instance's
        # topics and its new_message() stamps the config-resolved identity element with
        # this instance token — so consumers know who/where every sample came from.
        self._instance = gg.instance(instance_id)

        # Standard southbound health metric (contract §5).
        MetricEmitter.define_metric(
            MetricBuilder.create("southbound_health").with_config(self._cm)
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
                self._publish("ExampleSignal", value)
                MetricEmitter.emit_metric("southbound_health", {"connectionState": 1.0, "readErrors": 0.0})
            except Exception as e:  # noqa: BLE001
                logger.error("[%s] poll failed: %s", self._id, e)
                MetricEmitter.emit_metric("southbound_health", {"connectionState": 0.0, "readErrors": 1.0})

    def _read_one(self):
        return 0  # TODO: read a real value from your protocol client

    def _publish(self, signal_name, value):
        body = {
            "device": {"adapter": ADAPTER, "instance": self._id, "endpoint": "TODO"},
            "signal": {"id": f"{self._id}/{signal_name}", "name": signal_name,
                       "address": {"signal": signal_name}},
            "samples": [{"value": value, "quality": "GOOD", "qualityRaw": "Good",
                         "sourceTs": None, "serverTs": _now_iso()}],
        }
        # Data-plane topic minted through the UNS builder — never hand-written:
        # ecv1/{device}/{component}/{instance}/data/{signalPath}. The channel token is
        # the sanitized signal name (the raw, stable id still travels in body
        # signal.id); the canonical signalId->channel mapping is finalized in Phase 5
        # (D-U15).
        topic = self._instance.uns().topic(UnsClass.DATA, ConfigManager.sanitize(signal_name))
        msg = (
            self._instance.new_message("SouthboundSignalUpdate", "1.0")
            .with_payload(body)
            .build()
        )
        self._gg.get_messaging().publish(topic, msg)

    # NOTE — command surface (on-demand read / write / control, contract §2.2): keep
    # serving it on this instance's `write.topic` / `read.topic` subscriptions for now.
    # The standardized southbound command family moves to this instance's UNS command
    # inbox (`ecv1/{device}/{component}/{instance}/cmd/sb/{verb}`) in Phase 5 (M9).

    def stop(self):
        self._stop.set()
        # TODO: close your protocol client.
