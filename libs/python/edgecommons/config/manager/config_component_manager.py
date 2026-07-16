"""The ``CONFIG_COMPONENT`` config source: fetches this component's configuration
from an external configuration-manager component over the UNS config rendezvous
(UNS-CANONICAL-DESIGN §4.3, D-U19 Flow A) and receives pushed updates on the
component's own command inbox.

Wire contract (a convention shared with the config server):

- **Flow A — GET**: a request to ``ecv1/{device}/config/cmd/get-configuration``
  (with ``{device}`` = the sanitized resolved thing name). ``config`` is a
  **reserved-by-convention logical component name** — the config server is the sole
  subscriber and replies via ``reply_to`` with the configuration as the message body.
  The rendezvous is at **component scope** (D-U28 — no instance token). Because this
  request runs during config bootstrap — *before* the component identity is resolved —
  it carries no envelope identity; the requester **self-identifies in the body** with
  ``{"component": "<short name>"}`` (§1.5).
- **set-config push**: the server pushes a fire-and-forget ``cmd`` (no ``reply_to`` —
  a notification-style command) to the component's own inbox
  ``ecv1/{device}/{component}/cmd/set-config`` (with ``{component}`` = the sanitized
  short component name, component scope, D-U28 — no instance token); the body is the
  new configuration, applied via ``configuration_changed``.

The topics are minted locally from the resolved thing name and the component short
name (the same inputs identity resolution later uses) — never from
``get_component_identity()``/``Uns``, which do not exist until this manager has loaded
the config. Both tokens pass through the normative UNS token sanitizer
(``ConfigManager.sanitize``). These are ``cmd``-class topics — not library-reserved —
so they publish through the ordinary messaging surface (no reserved seam) and pass
the reserved-topic guard.
"""
import json
import logging


from edgecommons.messaging.errors import RequestTimeoutError
from edgecommons.messaging.messaging_client import MessagingClient
from edgecommons.config.manager.config_manager import ConfigManager
from edgecommons.messaging.message_builder import MessageBuilder
from edgecommons.messaging.message import Message

logger = logging.getLogger("ConfigComponentManager")


class ConfigComponentManager(ConfigManager):
    #: Flow-A GET request topic (§4.3): the config server's rendezvous under the
    #: reserved-by-convention logical component name ``config`` (component scope, D-U28
    #: — no instance token).
    GET_TOPIC_TEMPLATE = "ecv1/{device}/config/cmd/get-configuration"

    #: The pushed ``set-config`` command's topic — this component's OWN inbox (§4.3):
    #: the server-to-component push replacing the legacy ``.../updated`` subscription
    #: (component scope, D-U28 — no instance token).
    SET_CONFIG_TOPIC_TEMPLATE = "ecv1/{device}/{component}/cmd/set-config"

    def load_and_apply_config(self, topic: str, message: Message):
        logger.info("set-config push received")
        config = message.get_body()
        self.configuration_changed(config)

    def __init__(
        self,
        thing_name: str,
        component_name: str,
        platform=None,
        candidate_validators=None,
        validation_timeout_secs=5.0,
    ):
        super().__init__(
            component_name,
            thing_name,
            platform=platform,
            candidate_validators=candidate_validators,
            validation_timeout_secs=validation_timeout_secs,
        )
        # Mint the UNS tokens locally (identity is not resolved yet): device =
        # sanitized resolved thing name, component = sanitized short name, mirroring
        # the {ThingName}/{ComponentName} template semantics and §1.5 steps 4-5.
        device_token = ConfigManager.sanitize(thing_name)
        # The sanitized short component name — the body self-identification token
        # (§1.5). get_component_name() already reduced the full name to its short form.
        self._component_token = ConfigManager.sanitize(self.get_component_name())
        self._get_topic = self.GET_TOPIC_TEMPLATE.replace("{device}", device_token)
        self._set_config_topic = (
            self.SET_CONFIG_TOPIC_TEMPLATE
            .replace("{device}", device_token)
            .replace("{component}", self._component_token)
        )
        self._config_source = (
            f"Config Manager Component (get: {self._get_topic},"
            f" set-config inbox: {self._set_config_topic})"
        )
        self._config_provider_family = "CONFIG_COMPONENT"
        self.init()
        # Push delivery is externally observable provider activity, so it begins only
        # after the INITIAL generation committed and requires positive acknowledgement.
        MessagingClient.subscribe_acknowledged(
            self._set_config_topic,
            self.load_and_apply_config,
            timeout_secs=10.0,
        )
        self._push_subscription_started = True

    def close(self) -> None:
        if getattr(self, "_push_subscription_started", False):
            try:
                MessagingClient.unsubscribe(self._set_config_topic)
            except Exception as exc:  # noqa: BLE001 - shutdown remains best effort
                logger.debug("Failed to unsubscribe CONFIG_COMPONENT push: %s", exc)
            self._push_subscription_started = False

    def _load_configuration(self) -> dict:
        # This bootstrap request carries the framework-owned request() deadline
        # (UNS-CANONICAL-DESIGN §5; the provider's built-in 30 s, since the
        # config-model default is not loaded yet). When the deadline fires it settles
        # the request — the reply subscription is unsubscribed and Iou.get() raises
        # RequestTimeoutError — so a retry must issue a FRESH request (waiting again
        # on the settled Iou could never succeed). Both timeout signals (the framework
        # deadline raising RequestTimeoutError and get()'s own expiry when the
        # deadline is disabled) take the same 3-attempt retry path.
        attempt_count = 0
        while True:
            # The requester self-identifies in the BODY (§1.5): during bootstrap the
            # component identity is not resolved, so the envelope carries no identity
            # element (built without a config-bound builder) — the config server
            # routes on {"component"} instead.
            request_payload = {"component": self._component_token}
            request = MessageBuilder.create("GetConfiguration", "1.0") \
                .with_payload(request_payload) \
                .build()
            iou = MessagingClient.request(self._get_topic, request)
            try:
                done, reply = iou.get(timeout=30)
            except RequestTimeoutError as e:
                # The framework deadline fired (and already cleaned up the reply
                # subscription).
                attempt_count = self._on_timeout(attempt_count, e)
                continue
            if not done:
                # get() expired before any framework deadline (e.g. deadline
                # disabled): settle and clean up the abandoned request before
                # re-issuing.
                MessagingClient.cancel_request(iou)
                attempt_count = self._on_timeout(
                    attempt_count,
                    TimeoutError(f"no reply within 30 s on '{self._get_topic}'"),
                )
                continue

            body = {}
            if isinstance(reply, Message):
                body = reply.get_body()
                if isinstance(body, str):
                    body = json.loads(body)
            logger.debug("Fetched body of message as %s", body)
            return body

    @staticmethod
    def _on_timeout(attempt_count: int, error: Exception) -> int:
        """The shared 3-attempt timeout policy: increments, raises on the 3rd attempt,
        else warns."""
        attempt_count += 1
        if attempt_count == 3:
            logger.critical(
                f"Failed to retrieve configuration from configuration manager"
                f" component after {attempt_count} tries."
            )
            raise RuntimeError(
                f"Failed to retrieve configuration from configuration manager"
                f" component after {attempt_count} tries."
            ) from error
        logger.warning(
            f"Failed to retrieve configuration from configuration manager component."
            f"  Retrying ({attempt_count})"
        )
        return attempt_count
