"""The top-level ``identity`` envelope element of the unified namespace (UNS).

One immutable class serves as both the wire object and the component's resolved
identity (see ``ConfigManager.get_component_identity()``). It carries:

- ``hier`` — the ordered enterprise hierarchy (size >= 1); its **last entry is
  always the physical device**. There is no standalone ``device`` wire field —
  :attr:`MessageIdentity.device` is a computed accessor over the last entry.
- ``path`` — the precomputed ``'/'``-join of the ``hier`` values. The publisher is
  authoritative: on deserialize a present ``path`` is taken as-is, a missing one is
  recomputed.
- ``component`` — the publishing component's UNS token (the sanitized short name,
  i.e. the existing ``{ComponentName}`` semantics).
- ``instance`` — the per-message instance token, or ``None`` for component/global
  scope (D-U28); never a reserved class token when present.

Serialization (:meth:`MessageIdentity.to_dict`) emits the canonical member order
``hier, path, component, instance``. Deserialization (:meth:`MessageIdentity.from_dict`)
is deliberately lenient, mirroring the lenient envelope handling across all four
libraries: a malformed ``identity`` yields ``None`` plus a WARN log, and the message
still delivers.
"""
import logging
from typing import List, Optional

logger = logging.getLogger("MessageIdentity")


class HierEntry:
    """One level of the enterprise hierarchy: the level's configured ``level`` name
    and this deployment's ``value`` for it. Both parts must be non-empty strings."""

    __slots__ = ("level", "value")

    def __init__(self, level: str, value: str):
        if not level:
            raise ValueError("MessageIdentity hier entry level must be non-empty")
        if not value:
            raise ValueError(
                f"MessageIdentity hier entry value for level '{level}' must be non-empty"
            )
        self.level = level
        self.value = value

    def __eq__(self, other):
        return (
            isinstance(other, HierEntry)
            and self.level == other.level
            and self.value == other.value
        )

    def __hash__(self):
        return hash((self.level, self.value))

    def __repr__(self):
        return f"HierEntry(level={self.level!r}, value={self.value!r})"


class MessageIdentity:
    """The immutable UNS identity element: ``hier``/``path``/``component``/``instance``."""

    #: Class tokens an instance id may not equal (D-U28): keeping the component-scope
    #: and instance-scope UNS subscription templates disjoint, and letting the
    #: reserved-class guard locate the class unambiguously.
    RESERVED_CLASS_TOKENS = frozenset(
        {"state", "metric", "cfg", "log", "data", "evt", "cmd", "app"}
    )

    __slots__ = ("_hier", "_path", "_component", "_instance")

    def __init__(self, hier: List[HierEntry], component: str,
                 instance: Optional[str] = None, path: Optional[str] = None):
        """Creates a validated identity, precomputing ``path`` as the ``'/'``-join of
        the ``hier`` values (an explicit ``path`` — the wire value — is authoritative).

        :param hier: ordered hierarchy entries (non-empty; last entry = device)
        :param component: the component UNS token (non-empty)
        :param instance: the instance token, or ``None``/empty for component/global
            scope (D-U28); a present token may not be a reserved class token
        :param path: an explicit wire ``path`` (used by :meth:`from_dict`), or ``None``
        :raises ValueError: if ``hier`` is empty, ``component`` is empty, or ``instance``
            is a reserved class token
        """
        if not hier:
            raise ValueError("MessageIdentity hier must contain at least one entry")
        if not component:
            raise ValueError("MessageIdentity component must be non-empty")
        # D-U28: absent/empty ⇒ component scope (None); a present instance may not be a
        # reserved class token.
        if instance and instance in MessageIdentity.RESERVED_CLASS_TOKENS:
            raise ValueError(
                f"MessageIdentity instance '{instance}' must not be a reserved UNS"
                " class token"
            )
        self._hier = tuple(hier)
        self._path = path if path is not None else "/".join(e.value for e in self._hier)
        self._component = component
        self._instance = instance if instance else None

    @property
    def hier(self):
        """The immutable, ordered hierarchy entries (the last entry is the device)."""
        return self._hier

    @property
    def path(self) -> str:
        """The precomputed ``'/'``-join of the hierarchy values."""
        return self._path

    @property
    def component(self) -> str:
        """The component UNS token (the sanitized short name)."""
        return self._component

    @property
    def instance(self) -> Optional[str]:
        """The per-message instance token, or ``None`` for component/global scope (D-U28)."""
        return self._instance

    @property
    def device(self) -> str:
        """Computed accessor — the last ``hier`` entry's value. NOT a wire field: the
        device is inherent to the hierarchy (its deepest level), so it is never
        serialized separately."""
        return self._hier[-1].value

    def with_instance(self, instance: Optional[str]) -> "MessageIdentity":
        """Returns a copy of this identity with a different per-message instance token,
        or component scope when ``instance`` is ``None``/empty (D-U28).

        :param instance: the instance token, or ``None``/empty for component/global scope
        :raises ValueError: if ``instance`` is a reserved class token
        """
        return MessageIdentity(list(self._hier), self._component, instance, self._path)

    def to_dict(self) -> dict:
        """Serializes this identity to its wire form, in the canonical member order
        ``hier, path, component, instance`` — the ``instance`` key is OMITTED when the
        identity is component-scoped (D-U28)."""
        result = {
            "hier": [{"level": e.level, "value": e.value} for e in self._hier],
            "path": self._path,
            "component": self._component,
        }
        if self._instance is not None:
            result["instance"] = self._instance
        return result

    @staticmethod
    def from_dict(src) -> Optional["MessageIdentity"]:
        """Lenient wire-form parser: a missing/empty ``instance`` means component scope
        (absent, D-U28); a missing ``path`` is recomputed from the hier values
        (a present one is taken as-is — the publisher is authoritative); a malformed
        identity (non-dict, missing/empty/non-list ``hier``, malformed hier entries, or a
        missing ``component``) yields ``None`` plus a WARN log so the enclosing message
        still delivers.
        """
        if not isinstance(src, dict):
            logger.warning(
                "Malformed message identity: 'identity' is not an object; dropping identity"
            )
            return None
        try:
            hier_raw = src.get("hier")
            if not isinstance(hier_raw, list) or not hier_raw:
                logger.warning(
                    "Malformed message identity: 'hier' missing, not an array, or empty;"
                    " dropping identity"
                )
                return None
            hier = []
            for entry in hier_raw:
                if not isinstance(entry, dict):
                    logger.warning(
                        "Malformed message identity: hier entry is not an object; dropping identity"
                    )
                    return None
                level = _as_non_empty_str(entry.get("level"))
                value = _as_non_empty_str(entry.get("value"))
                if level is None or value is None:
                    logger.warning(
                        "Malformed message identity: hier entry missing level/value;"
                        " dropping identity"
                    )
                    return None
                hier.append(HierEntry(level, value))
            component = _as_non_empty_str(src.get("component"))
            if component is None:
                logger.warning(
                    "Malformed message identity: 'component' missing or empty; dropping identity"
                )
                return None
            path = _as_non_empty_str(src.get("path"))          # None -> recomputed
            instance = _as_non_empty_str(src.get("instance"))  # None -> component scope (D-U28)
            return MessageIdentity(hier, component, instance, path)
        except Exception as e:  # noqa: BLE001 - lenient by design (mirrors Java)
            logger.warning(f"Malformed message identity ({e}); dropping identity")
            return None

    def __eq__(self, other):
        return (
            isinstance(other, MessageIdentity)
            and self._hier == other._hier
            and self._path == other._path
            and self._component == other._component
            and self._instance == other._instance
        )

    def __hash__(self):
        return hash((self._hier, self._path, self._component, self._instance))

    def __repr__(self):
        return (
            f"MessageIdentity(hier={list(self._hier)!r}, path={self._path!r},"
            f" component={self._component!r}, instance={self._instance!r})"
        )

    def __str__(self):
        import json
        return json.dumps(self.to_dict())


def _as_non_empty_str(value) -> Optional[str]:
    """The value as a non-empty string, or ``None`` if absent/non-string/empty."""
    if not isinstance(value, str) or value == "":
        return None
    return value
