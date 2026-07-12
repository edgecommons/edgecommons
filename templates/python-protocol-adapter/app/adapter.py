"""<<COMPONENTNAME>> — southbound protocol-adapter skeleton (docs/SOUTHBOUND.md).

Replace the TODOs with your protocol client. One instance of this class runs per
``component.instances[]`` entry. It should connect to its source (retrying until up), then either
subscribe or poll it and republish value changes as ``SouthboundSignalUpdate`` messages on this
instance's UNS ``data`` topic (``ecv1/{device}/{component}/{instance}/data/{signalPath}``), and
serve the optional read/write/control command surface. See the OPC UA (subscribe) and Modbus
(poll) reference adapters for full implementations.

It also reports each device's health through :class:`LinkStatus` — the instance-connectivity
provider ``main.py`` registers, whose sample the library both pushes on every ``state`` keepalive
and returns from the built-in ``status`` verb.
"""
import logging
import threading
from datetime import datetime, timezone

from edgecommons.config.manager.config_manager import ConfigManager
from edgecommons.heartbeat.instance_connectivity import InstanceConnectivity
from edgecommons.metrics.metric_builder import MetricBuilder
from edgecommons.metrics.metric_emitter import MetricEmitter
from edgecommons.uns import UnsClass

logger = logging.getLogger("<<COMPONENTNAME>>")

ADAPTER = "example"   # TODO: your protocol id, e.g. "modbus" / "opcua"

#: This adapter's OWN vocabulary for a link's condition. A boolean says "not reachable"; it cannot
#: say whether we are still coming up, backing off after a failure, or switched off on purpose —
#: and an operator needs to know which.
CONNECTING = "CONNECTING"   # configured, not up yet — nothing has failed
ONLINE = "ONLINE"           # the session is up
BACKOFF = "BACKOFF"         # it failed; we are retrying


def _now_iso():
    return datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")


class LinkStatus:
    """One configured device's live link condition: written by its worker, read by the
    instance-connectivity provider.

    It is created for **every** configured device, at startup, before any worker runs — so a device
    that is configured but down is reported as ``CONNECTING``/``connected=False``, and can never be
    mistaken for a device that was never configured at all. That is the whole reason this class is
    not simply owned by the adapter object: the adapter object may not exist yet, or at all.
    """

    def __init__(self, instance_id, inst_cfg=None):
        inst_cfg = inst_cfg or {}
        self.instance_id = instance_id
        self.adapter = inst_cfg.get("adapter", ADAPTER)
        self.endpoint = (inst_cfg.get("connection") or {}).get("endpoint")
        # One tuple, swapped atomically: the sampling thread can never read a state from one moment
        # and a detail from another.
        self._snapshot = (CONNECTING, self.endpoint)

    def set(self, state, detail=None):
        """Record the link's condition. Call it where the truth changes: ``ONLINE`` when your client
        connects (or a read succeeds), ``BACKOFF`` when it fails."""
        self._snapshot = (state, detail or self.endpoint)

    def connectivity(self) -> InstanceConnectivity:
        """This device's entry in the ``state`` keepalive's ``instances[]`` and in the ``status``
        verb's reply.

        ``connected`` is the **normalized** flag — always present, so a console renders a health dot
        without knowing this protocol. ``state`` is the vocabulary above. ``attributes`` is the open
        bag for domain data (here: which protocol), so what only this adapter understands can never
        destabilize the two fields every consumer reads.
        """
        state, detail = self._snapshot
        return (
            InstanceConnectivity.of(self.instance_id, state == ONLINE, detail)
            .with_state(state)
            .with_attributes({"adapter": self.adapter})
        )


def link_statuses(config_manager) -> dict:
    """A :class:`LinkStatus` for every configured device — built before the workers start."""
    return {
        instance_id: LinkStatus(instance_id, config_manager.get_instance_config(instance_id))
        for instance_id in config_manager.get_instance_ids()
    }


class <<COMPONENTNAME>>:
    def __init__(self, gg, instance_id, link):
        self._gg = gg
        self._cm = gg.get_config_manager()
        self._id = instance_id
        self._inst = self._cm.get_instance_config(instance_id) or {}
        self._link = link
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
        # TODO: construct + connect your protocol client here (block/retry until connected), and
        # call self._link.set(ONLINE) once the session is up / self._link.set(BACKOFF, reason) when
        # a connect attempt fails.
        logger.info("[%s] starting", instance_id)

    def run(self):
        """Subscribe or poll the source and publish changes. (Poll loop shown.)"""
        interval = self._inst.get("pollIntervalMs", 1000) / 1000.0
        while not self._stop.wait(interval):
            try:
                value = self._read_one()                      # TODO: read from your device
                self._publish("ExampleSignal", value)
                # A completed read is the proof the link is up. The reported state and the health
                # metric move together, so the dot a console renders and the line an operator
                # charts can never disagree.
                self._link.set(ONLINE)
                MetricEmitter.emit_metric("southbound_health", {"connectionState": 1.0, "readErrors": 0.0})
            except Exception as e:  # noqa: BLE001
                logger.error("[%s] poll failed: %s", self._id, e)
                self._link.set(BACKOFF, str(e))
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
