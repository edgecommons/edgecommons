"""Per-instance connectivity reporting for the ``state`` keepalive (the #1c model).

A multi-connection component (an OPC UA adapter with several servers, a Modbus adapter
with several slaves, a file-replicator with several source directories) keeps its
identity, data and lifecycle under its ``main`` instance token — it does NOT mint a
separate UNS instance per connection. Instead it reports each connection's health here,
and the console renders it per-instance under the one component (no phantom "instance"
component that never heartbeats).

Register a provider with ``gg.set_instance_connectivity_provider(...)``; the ``main``
``state`` keepalive then carries an ``instances`` array of :class:`InstanceConnectivity`.
"""

from dataclasses import dataclass
from typing import Any, Callable, Dict, List, Optional


@dataclass(frozen=True)
class InstanceConnectivity:
    """One component instance's southbound/source connectivity.

    :param instance: the component instance / connection id (OPC UA server id, Modbus
        slave id, replication instance id); must be non-empty.
    :param connected: whether that instance's southbound/source is currently reachable.
    :param detail: an optional human detail (endpoint, or the down reason).
    """

    instance: str
    connected: bool
    detail: Optional[str] = None

    def __post_init__(self) -> None:
        if not self.instance or not self.instance.strip():
            raise ValueError("instance id must be non-empty")

    @staticmethod
    def of(instance: str, connected: bool, detail: Optional[str] = None) -> "InstanceConnectivity":
        """Convenience factory."""
        return InstanceConnectivity(instance, connected, detail)

    def to_dict(self) -> Dict[str, Any]:
        """The state-body element: ``{"instance": …, "connected": …[, "detail": …]}``."""
        d: Dict[str, Any] = {"instance": self.instance, "connected": self.connected}
        if self.detail and self.detail.strip():
            d["detail"] = self.detail
        return d


# A component-supplied source of per-instance connectivity, sampled each keepalive tick.
# A zero-arg callable returning the current per-instance connectivity; ``None``/empty omits
# the ``instances[]`` section. Must be cheap and non-blocking (sample a cached status) and
# must not raise (the heartbeat treats a raising provider as "no data this tick").
InstanceConnectivityProvider = Callable[[], Optional[List[InstanceConnectivity]]]
