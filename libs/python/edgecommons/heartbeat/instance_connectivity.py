"""Per-instance connectivity reporting for the ``state`` keepalive and the ``status``
verb (the #1c model).

A multi-connection component (an OPC UA adapter with several servers, a Modbus adapter
with several slaves, a file-replicator with several source directories) keeps its
identity, data and lifecycle at **component scope** (no instance token — D-U28) — it
does NOT mint a separate UNS instance per connection. Instead it reports each
connection's health here, and the console renders it per-instance under the one
component (no phantom "instance" component that never heartbeats).

Register a provider with ``gg.set_instance_connectivity_provider(...)``; the component's
``state`` keepalive then carries an ``instances`` array of :class:`InstanceConnectivity`.
The same sample answers both surfaces: it is pushed on every ``state`` keepalive tick,
and it is what the built-in ``status`` command verb
(:data:`edgecommons.command_inbox.STATUS`) returns when pulled. A component supplies the
data once; the library serves it both ways.

Mirrors ``libs/java/.../heartbeat/InstanceConnectivity.java`` (the Java canonical).
"""

from dataclasses import dataclass, replace
from typing import Any, Callable, Dict, List, Mapping, Optional


@dataclass(frozen=True)
class InstanceConnectivity:
    """One component instance's southbound/source connectivity.

    :param instance: the component instance / connection id (OPC UA server id, Modbus
        slave id, replication instance id); must be non-empty.
    :param connected: whether that instance's southbound/source is currently reachable —
        the **normalized** flag every consumer can read, so a console can render a health
        dot for any component without knowing that component's vocabulary.
    :param detail: an optional human detail (endpoint, or the down reason).
    :param state: optional — the component's **own** vocabulary for the richer condition
        (``ONLINE`` / ``CONNECTING`` / ``BACKOFF`` / ``DISABLED`` …). A boolean cannot
        distinguish "reconnecting" from "administratively disabled"; this can.
    :param attributes: optional open bag of domain data (a camera's capabilities and last
        error, an OPC UA server's session id …). Deliberately unconstrained: it is where a
        component puts what only it understands, **without** destabilizing the fields above
        that everyone relies on. Copied defensively.
    """

    instance: str
    connected: bool
    detail: Optional[str] = None
    state: Optional[str] = None
    attributes: Optional[Dict[str, Any]] = None

    def __post_init__(self) -> None:
        if not self.instance or not self.instance.strip():
            raise ValueError("instance id must be non-empty")
        if self.attributes is not None:
            if not isinstance(self.attributes, Mapping):
                raise ValueError("attributes must be a mapping")
            # Defensive copy: a later mutation of the caller's dict must not leak in.
            object.__setattr__(self, "attributes", dict(self.attributes))

    @staticmethod
    def of(instance: str, connected: bool, detail: Optional[str] = None) -> "InstanceConnectivity":
        """Convenience factory (the pre-``state``/``attributes`` surface, retained)."""
        return InstanceConnectivity(instance, connected, detail)

    def with_state(self, state: Optional[str]) -> "InstanceConnectivity":
        """A copy carrying the component's own condition token."""
        return replace(self, state=state)

    def with_attributes(self, attributes: Optional[Mapping[str, Any]]) -> "InstanceConnectivity":
        """A copy carrying domain-specific attributes."""
        return replace(self, attributes=dict(attributes) if attributes is not None else None)

    def to_dict(self) -> Dict[str, Any]:
        """The wire element:
        ``{"instance": …, "connected": …[, "state": …][, "detail": …][, "attributes": {…}]}``.

        Optional members are omitted rather than emitted null, so the common two-field case
        stays small on a keepalive that ships every 5 seconds per component.
        """
        d: Dict[str, Any] = {"instance": self.instance, "connected": self.connected}
        if self.state and self.state.strip():
            d["state"] = self.state
        if self.detail and self.detail.strip():
            d["detail"] = self.detail
        if self.attributes:
            d["attributes"] = dict(self.attributes)
        return d


# A component-supplied source of per-instance connectivity, sampled each keepalive tick
# and by the built-in ``status`` verb. A zero-arg callable returning the current
# per-instance connectivity; ``None``/empty omits the ``instances[]`` section. Must be
# cheap and non-blocking (sample a cached status) and should not raise (the heartbeat's
# sampling seam treats a raising provider as "no data this sample").
InstanceConnectivityProvider = Callable[[], Optional[List[InstanceConnectivity]]]
