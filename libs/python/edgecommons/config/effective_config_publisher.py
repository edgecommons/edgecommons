"""The library-owned ``cfg`` publisher (UNS-CANONICAL-DESIGN §4.3): announces the
component's effective (redacted) configuration on
``ecv1/{device}/{component}/main/cfg`` — once at startup (after initialization
completes) and again on every configuration change. The body is
``{"config": <effective config, redacted>}``; the ``cfg`` class is reserved, so the
publish goes through the privileged ``MessagingClient._publish_reserved`` seam.
(This is the push half only — the ``republish-cfg`` pull verb lands in a later phase.)

**Redaction v1** (§4.3): ``$secret`` references are never resolved (the raw config is
published as-is, so a ``{"$secret": ...}`` ref stays a ref); every value under a
``credentials`` key inside the top-level ``messaging`` section, and every value of a
key named ``password`` or ``pin`` (case-insensitive) anywhere, is replaced with
``"***"``.
"""
import copy
import logging
from typing import Optional

from edgecommons.config.manager.configuration_change_listener import ConfigurationChangeListener
from edgecommons.uns import Uns, UnsClass

logger = logging.getLogger("EffectiveConfigPublisher")

#: The cfg announcement's envelope header name (§4.3).
CFG_MESSAGE_NAME = "cfg"
CFG_MESSAGE_VERSION = "1.0"
#: The redaction placeholder.
REDACTED = "***"


class EffectiveConfigPublisher(ConfigurationChangeListener):
    """Announces the effective (redacted) configuration on the UNS ``cfg`` topic."""

    def __init__(self, config_manager, messaging_client):
        """Creates the publisher and registers it as a configuration-change listener
        (each hot reload republishes the effective config). Call :meth:`publish_now`
        for the startup announcement.

        :param config_manager: the component's config manager (identity + effective
            config source)
        :param messaging_client: the messaging handle (the ``MessagingClient`` class)
            whose privileged seam performs the publish
        """
        if config_manager is None:
            raise ValueError("config_manager must not be None")
        if messaging_client is None:
            raise ValueError("messaging_client must not be None")
        self._config_manager = config_manager
        self._messaging_client = messaging_client
        # WARN-once flag for the no-resolved-identity (test/subclass bring-up) case.
        self._warned_no_identity = False
        config_manager.add_config_change_listener(self)

    def publish_now(self) -> None:
        """Publishes the effective (redacted) configuration to the component's UNS
        ``cfg`` topic. Best-effort: any failure is logged and swallowed — a cfg
        announcement must never crash the component. No-op (WARN once) when the
        component identity is not resolved (mock/test bring-up)."""
        try:
            identity = self._config_manager.get_component_identity()
            if identity is None:
                if not self._warned_no_identity:
                    self._warned_no_identity = True
                    logger.warning(
                        "No resolved component identity - the effective-config"
                        " publisher is disabled"
                    )
                return
            redacted = self.redacted_effective_config()
            if redacted is None:
                logger.warning("No effective configuration available - skipping cfg publish")
                return

            # Import here to avoid circular imports.
            from edgecommons.messaging.message_builder import MessageBuilder

            topic = Uns(identity, self._config_manager.is_topic_include_root()).topic(UnsClass.CFG)
            body = {"config": redacted}
            cfg_message = MessageBuilder.create(CFG_MESSAGE_NAME, CFG_MESSAGE_VERSION) \
                .with_payload(body) \
                .with_config(self._config_manager) \
                .build()
            self._messaging_client._publish_reserved(topic, cfg_message)
            logger.debug(f"Published effective (redacted) configuration on '{topic}'")
        except Exception as e:  # noqa: BLE001 - best-effort by design
            logger.warning(f"Effective-config publish failed: {e}")

    def on_configuration_change(self, configuration) -> bool:
        self.publish_now()
        return True

    def redacted_effective_config(self) -> Optional[dict]:
        """The current effective configuration, redacted (redaction v1) — the single
        snapshot source shared by the ``cfg`` push (:meth:`publish_now`) and the
        ``get-configuration`` command verb's reply (DESIGN-uns §9.5 Flow B), so both
        surfaces always agree byte-for-byte.

        :returns: the redacted deep copy of the effective config, or ``None`` when no
            effective configuration is available (mock/test bring-up, or before any
            config was applied)
        """
        effective_config = self._config_manager.get_effective_config()
        return None if effective_config is None else redact(effective_config)


def redact(config: dict) -> dict:
    """Redaction v1 (§4.3) over a deep copy of the effective config: every value of a
    key named ``password`` or ``pin`` (case-insensitive, anywhere) and every value of
    a ``credentials`` key at any depth inside the top-level ``messaging`` section
    becomes the string ``"***"``. ``$secret`` refs are untouched (they are never
    resolved here, so no secret material exists to leak).

    :param config: the effective config (not mutated)
    :returns: the redacted deep copy
    """
    redacted = copy.deepcopy(config)
    _redact_object(redacted, in_messaging=False, top_level=True)
    return redacted


def _redact_object(obj: dict, in_messaging: bool, top_level: bool) -> None:
    """Recursive redaction walk. ``in_messaging`` is true anywhere under the
    **top-level** ``messaging`` section (the ``messaging.*.credentials`` rule);
    ``top_level`` is true only for the config root, so a nested ``messaging`` key
    elsewhere does not trigger the credentials rule."""
    for key in list(obj.keys()):
        lower = key.lower() if isinstance(key, str) else key
        if lower in ("password", "pin") or (in_messaging and lower == "credentials"):
            obj[key] = REDACTED
            continue
        value = obj[key]
        if isinstance(value, dict):
            _redact_object(value, in_messaging or (top_level and key == "messaging"), False)
        elif isinstance(value, list):
            for item in value:
                if isinstance(item, dict):
                    _redact_object(item, in_messaging, False)
