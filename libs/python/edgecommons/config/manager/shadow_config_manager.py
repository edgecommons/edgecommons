import json
import logging
import re

from awsiot.greengrasscoreipc.model import ReceiveMode, SubscriptionResponseMessage
from edgecommons.config.manager.config_manager import ConfigManager

logger = logging.getLogger("ShadowConfigManager")


def _sanitize_shadow_name(name: str) -> str:
    """Sanitize a default shadow name to AWS IoT's allowed set (``[A-Za-z0-9:_-]``);
    any other character (notably the ``.`` in a component name) becomes ``_``.
    Identical across the Java/Python/Rust/TS libraries so they agree on the same
    shadow. Applied only to the component-name default; an explicit name is verbatim.
    """
    return re.sub(r"[^A-Za-z0-9:_-]", "_", name)


class ShadowConfigManager(ConfigManager):
    _SHADOW_TOPIC_TEMPLATE = "$aws/things/{}/shadow/name/{}/"
    _ALL_SHADOW_TOPIC_TEMPLATE = "$aws/things/{}/shadow/name/{}/+/+"
    _DEFAULT_CONFIGURATION = {
        "logging": {},
        "tags": {},
        "heartbeat": {},
        "component": {"global": {}, "instances": []},
    }

    def __init__(
        self,
        thing_name: str,
        component_name: str,
        shadow_name: str,
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
        self._shadow_name = (
            shadow_name if shadow_name is not None else _sanitize_shadow_name(component_name)
        )
        self._config_source = f"Named Shadow (shadow name: {self._shadow_name})"
        self._config_provider_family = "SHADOW"
        from edgecommons import MessagingClient

        self._ipc_client = (
            MessagingClient.get_native_client()
        )  # GreengrassCoreIPCClientV2()
        self._shadow_topic_prefix = ShadowConfigManager._SHADOW_TOPIC_TEMPLATE.format(
            self.get_thing_name(), self._shadow_name
        )
        self._shadow_subscription_operation = None
        self.init()
        # A candidate must be reported only after ``init`` has schema-validated it,
        # run every pre-commit validator, and installed the accepted snapshot.  In
        # particular, a rejected initial candidate must never become externally
        # visible as this component's reported configuration.
        self._report_updated_configuration(self.get_effective_config())
        # Provider activity starts only after the INITIAL candidate committed.
        self._subscribe_to_shadow_topics()

    def _subscribe_to_shadow_topics(self):
        logger.debug("Subscribing to shadow topics")
        try:
            shadow_update_delta_topic = (
                ShadowConfigManager._ALL_SHADOW_TOPIC_TEMPLATE.format(
                    self.get_thing_name(), self._shadow_name
                )
            )
            _, operation = self._ipc_client.subscribe_to_topic(
                topic=shadow_update_delta_topic,
                receive_mode=ReceiveMode.RECEIVE_MESSAGES_FROM_OTHERS,
                on_stream_closed=None,
                on_stream_error=None,
                on_stream_event=self._on_shadow_event,
            )
            self._shadow_subscription_operation = operation
        except Exception as e:
            logger.error(f"Failed to subscribe to shadow topics: {e}")

    def close(self) -> None:
        operation = self._shadow_subscription_operation
        self._shadow_subscription_operation = None
        if operation is not None:
            try:
                operation.close()
            except Exception as exc:  # noqa: BLE001 - shutdown remains best effort
                logger.debug("Failed to close shadow config subscription: %s", exc)

    def _on_shadow_event(self, event: SubscriptionResponseMessage) -> None:
        payload_str = str(event.binary_message.message, "utf-8")
        topic_parts = event.binary_message.context.topic.split("/")
        action = topic_parts[len(topic_parts) - 2]
        result = topic_parts[len(topic_parts) - 1]
        logger.debug(
            f"Received shadow message for shadow action '{action}' result '{result}'. Payload: {payload_str}"
        )

        if action == "get" and result == "rejected":
            logger.warning(
                f"Named shadow document {self._shadow_name} does not exist. Creating default configuration."
            )
            self._report_updated_configuration(
                ShadowConfigManager._DEFAULT_CONFIGURATION
            )
        elif action == "update" and result == "delta":
            payload_json = json.loads(payload_str)
            desired_doc = payload_json["state"]
            if desired_doc is not None:
                logger.debug(f"Desired document: {desired_doc}")
                component_config = json.loads(desired_doc["ComponentConfig"])
                # Never acknowledge a rejected candidate as the component's
                # externally visible reported configuration.  The configuration
                # manager invokes listeners only after this returns true, so the
                # reported document and every applied listener observe one accepted
                # generation.
                if self.configuration_changed(component_config):
                    self._report_updated_configuration(self.get_effective_config())
        # else:
        #     logger.info(f"Received message for shadow action '{action}' result '{result}'.")

    def _load_configuration(self) -> dict:
        logger.debug(f"Loading configuration from named shadow ('{self._shadow_name}')")
        # ``ConfigManager.init`` owns the pre-commit validation and atomic install.
        # Reporting here would publish an unvalidated source candidate before that
        # process can reject it.
        return self._get_configuration()

    def _report_updated_configuration(self, config: dict) -> None:
        shadow_doc = {
            "state": {
                "reported": {
                    "ComponentConfig": json.dumps(
                        config, indent=None, separators=(",", ":")
                    )
                }
            }
        }
        logger.debug(
            f"Reporting updated configuration to named shadow document '{self._shadow_name}': {shadow_doc}"
        )
        try:
            self._ipc_client.update_thing_shadow(
                thing_name=self.get_thing_name(),
                shadow_name=self._shadow_name,
                payload=json.dumps(shadow_doc).encode("utf-8"),
            )
        except Exception as e:
            logger.error(f"Failed to report updated configuration: {e}")

    def _get_configuration(self):
        try:
            response = self._ipc_client.get_thing_shadow(
                thing_name=self.get_thing_name(), shadow_name=self._shadow_name
            )
            if response.payload is not None and len(response.payload) > 0:
                payload = str(response.payload, "utf-8")
                payload_json = json.loads(payload)
                state_doc = payload_json["state"]
                if "desired" in state_doc:
                    return json.loads(state_doc["desired"]["ComponentConfig"])
                elif "reported" in state_doc:
                    return json.loads(state_doc["reported"]["ComponentConfig"])
            else:
                logger.warning(
                    f"Named shadow document '{self._shadow_name}' does not exist or is empty"
                )
            return None
        except Exception as e:
            logger.error(f"_get_configuration: Failed to get configuration: {str(e)}")
            return None
