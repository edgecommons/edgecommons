import json
import logging
import time
from abc import ABC
from random import random

from awsiot.greengrasscoreipc.model import QOS
from edgecommons.utils.iou import Iou
from edgecommons.metrics.metric_emitter import MetricEmitter
from edgecommons.metrics.metric_builder import MetricBuilder
from edgecommons.config.manager.configuration_change_listener import (
    ConfigurationChangeListener,
)
from edgecommons.messaging.errors import RequestTimeoutError
from edgecommons.messaging.message import Message
from edgecommons.messaging.message_builder import MessageBuilder
from edgecommons.messaging.messaging_client import MessagingClient
from edgecommons.uns import UnsClass

logger = logging.getLogger("GreengrassApp")

# Config key (under component.global) naming the secret the component reads; the default is a
# self-seeded demo secret so the example runs with no external provisioning.
DEMO_SECRET_KEY = "demo_secret"
# Default secret name when component.global.demo_secret is absent.
DEFAULT_DEMO_SECRET = "skeleton/demo-secret"


# This sample application subscribes to messages on its unified-namespace (UNS) `app`
# topic and then publishes a message every n seconds on that topic, where "n" comes
# from the app specific configuration section in the config file/recipe. The message
# is output to the log. All topics are minted through `gg.uns()` (never hand-written),
# so they carry the component's config-resolved identity
# (ecv1/{device}/{component}/{instance}/{class}/...). The application inherits
# configuration management, heartbeats (the automatic `state` keepalive), logging and
# switching between local MQTT and GG IPC from edgecommons.


class GreengrassApp(ConfigurationChangeListener, ABC):
    def __init__(self, gg, streams=None):
        super().__init__()
        self._gg = gg
        self._config_manager = gg.get_config_manager()
        self._config_manager.add_config_change_listener(self)
        # UNS topics, minted from the config-resolved identity (topic.includeRoot-aware).
        # `app` is the free application class; `cmd/echo` is this component's own
        # command-inbox verb used by the request/reply demo below.
        self._hello_topic = gg.uns().topic(UnsClass.APP, "hello-world")
        self._request_topic = gg.uns().topic(UnsClass.CMD, "echo")
        global_config = self._config_manager.get_global_config()
        self._publish_interval = (
            global_config["publish_interval"]
            if "publish_interval" in global_config
            else 5
        )
        # Durable telemetry stream handle (None unless the config has a `streaming` section and
        # a stream named "telemetry"). The publish loop appends each message here; the library's
        # export engine drains it to the configured sink (Kinesis) independently.
        self._stream = None
        if streams is not None:
            try:
                self._stream = streams.stream("telemetry")
                logger.info("Telemetry streaming enabled (stream 'telemetry')")
            except Exception as e:
                logger.warning(f"stream 'telemetry' unavailable; streaming disabled: {e}")
        self.define_metric()

    @staticmethod
    def demonstrate_credentials(gg):
        """Demonstrate encrypted-vault secret access via ``gg.get_credentials()``.

        Reads a named secret from the encrypted local vault and uses it -- without ever logging
        the value. Runs once at startup. In production the secret arrives via central sync (AWS
        Secrets Manager over TES, with a ``credentials.central`` config) or out-of-band
        provisioning; here, so the example is self-contained, we seed a demo value locally on
        first run if it is absent. Any vault error is logged and swallowed (non-fatal).
        """
        try:
            creds = gg.get_credentials()
            if creds is None:
                logger.info("no `credentials` config section; secret access demo disabled")
                return

            global_config = gg.get_config_manager().get_global_config()
            name = global_config.get(DEMO_SECRET_KEY, DEFAULT_DEMO_SECRET)

            # Seed a demo secret on first run (in production: central sync / provisioning).
            if not creds.exists(name):
                demo = json.dumps(
                    {"username": "svc-account", "password": "demo-secret-value"}
                ).encode("utf-8")
                version = creds.put(name, demo)
                logger.info(
                    "seeded demo secret (production: provided via central sync / "
                    f"provisioning) secret={name} version={version}"
                )

            # Read it back and use it -- logging only non-sensitive facts, never the value.
            s = creds.get(name)
            if s is None:
                logger.warning(f"secret not found after seeding (unexpected) secret={name}")
                return
            logger.info(
                f"credential access OK (value redacted) secret={name} "
                f"bytes={len(s.bytes())} source={s.source}"
            )

            # Demonstrate a typed view; log only the non-secret username.
            ba = creds.get_basic_auth(name)
            if ba is not None:
                logger.info(
                    f"parsed basic-auth view (password redacted) secret={name} "
                    f"username={ba.username}"
                )
        except Exception as e:
            logger.warning(f"vault error; skipping secret demo: {e}")

    @staticmethod
    def demonstrate_parameters(gg):
        """Demonstrate externalized-parameter access via ``gg.get_parameters()``.

        Reads a couple of non-secret configuration parameters from the offline-first parameter
        cache and logs the resolved values. The example config uses the ``env`` source, so this
        needs no AWS and no external provisioning -- the values come from environment variables
        (e.g. ``GG_PARAM_SKELETON_REGION``). Runs once at startup. A secure parameter's value would
        never be logged. Any parameter error is logged and swallowed (non-fatal).
        """
        try:
            params = gg.get_parameters()
            if params is None:
                logger.info("no `parameters` config section; parameter access demo disabled")
                return

            # A plain string parameter.
            region = params.get("/skeleton/region")
            logger.info(f"parameter access OK /skeleton/region={region}")

            # A typed (integer) parameter.
            pool_size = params.get_int("/skeleton/poolSize")
            logger.info(f"parameter access OK /skeleton/poolSize={pool_size}")

            st = params.stats()
            logger.info(
                f"parameters steady-state source={st.source} count={st.parameter_count}"
            )
        except Exception as e:
            logger.warning(f"parameter error; skipping parameter demo: {e}")

    def ipc_hello_world_handler(self, topic: str, msg: Message):
        # Every config-built message carries the publisher's identity element —
        # consumers read who/where it came from without parsing topics.
        sender = msg.get_identity()
        sender_path = sender.path if sender is not None else "<no identity>"
        logger.info(
            f"Received an ipc hello world message on topic {topic} from {sender_path}: "
            f"{msg.get_body()['msg_id']}"
        )
        time.sleep(5)
        logger.info(
            f"#### Received an ipc hello world message on topic {topic}: {msg.get_body()['msg_id']}"
        )

    def iot_core_hello_world_handler(self, topic: str, msg: Message):
        logger.info(
            f"Received an iot core hello world message on topic {topic}: {msg.get_body()['msg_id']}"
        )
        time.sleep(5)
        logger.info(
            f"Received an iot core hello world message on topic {topic}: {msg.get_body()['msg_id']}"
        )

    def request_callback(self, topic: str, request: Message):
        logger.info(f"Received request message [{topic}]: {request.get_body()['msg_id']}")
        reply_payload = {
            "reply_message": "I have received your request and have replied with this message"
        }
        reply = (
            MessageBuilder.create("ReplyTest", "1.0")
            .with_payload(reply_payload)
            .with_config(self._config_manager)
            .build()
        )
        time.sleep(request.get_body()["wait_time"])
        logger.info(f"Publishing reply message {request.get_body()['msg_id']}")
        MessagingClient.reply(request, reply)

    def publish_request(self, msg_id: str, execution_time: float) -> Iou:
        logger.info(f"Publishing reqeust message {msg_id}")
        request_payload = {"msg_id": msg_id, "wait_time": execution_time}
        request = (
            MessageBuilder.create("RequestTest", "1.0")
            .with_payload(request_payload)
            .with_config(self._config_manager)
            .build()
        )
        return MessagingClient.request(self._request_topic, request)

    def wait_for_reply(self, msg_instance: str, iou: Iou, timeout: float):
        logger.info(f"Waiting for reply for {msg_instance}")
        try:
            done, reply = iou.get(timeout)
        except RequestTimeoutError as e:
            # The framework-owned request deadline (messaging.requestTimeoutSeconds,
            # default 30 s) fired: the library already unsubscribed the reply topic and
            # removed the pending entry — a retry must issue a FRESH request.
            logger.warning(f"Request for {msg_instance} hit the framework deadline: {e}")
            return
        if done is False:
            # Our own (shorter) wait elapsed but the request is still pending.
            logger.warning(
                f"Reply for {msg_instance} timed out (took more than {timeout} seconds). Cancelling."
            )
            MessagingClient.cancel_request(reply)
        else:
            logger.info(f"...Received reply for {msg_instance}: {reply.dumps()}")

    def define_metric(self):
        metric = (
            MetricBuilder.create("performance")
            .with_config(self._config_manager)
            .add_measure("latency", "Milliseconds", 1)
            .build()
        )
        MetricEmitter.define_metric(metric)
        return metric

    def run(self):
        # Log the resolved UNS identity once at startup: the full hierarchy path (from the
        # top-level `hierarchy` + `identity` config; the last level is always the resolved
        # thing name — Downward API on KUBERNETES, -t/--thing or AWS_IOT_THING_NAME
        # elsewhere). Logging it here (after logging is configured) makes the resolved
        # identity observable in pod/container logs.
        identity = self._config_manager.get_component_identity()
        logger.info(
            f"Component identity: path={identity.path} component={identity.component}"
            f" (thing name: {self._config_manager.get_thing_name()})"
        )
        i = 1
        try:
            measure_values = {}
            MessagingClient.subscribe(
                self._hello_topic, self.ipc_hello_world_handler, True
            )
            # Non-fatal: setups without an IoT Core transport (e.g. a local-only MQTT broker)
            # skip the IoT Core bridge instead of failing the whole component.
            try:
                MessagingClient.subscribe_northbound(
                    self._hello_topic,
                    self.iot_core_hello_world_handler,
                    QOS.AT_LEAST_ONCE,
                )
            except Exception as e:
                logger.warning(f"IoT Core unavailable; skipping IoT Core subscribe: {e}")
            MessagingClient.subscribe(self._request_topic, self.request_callback)

            iou_1 = self.publish_request(msg_id="1", execution_time=0)
            iou_2 = self.publish_request(msg_id="2", execution_time=1)
            iou_3 = self.publish_request(msg_id="3", execution_time=5)

            self.wait_for_reply("iou_1", iou_1, 1)
            self.wait_for_reply("iou_3", iou_3, 3)
            self.wait_for_reply("iou_2", iou_2, 2)

            while True:
                test_message = (
                    MessageBuilder.create("hello_world", "1.0.0")
                    .with_payload({"msg_id": i, "message": "Hello World Python"})
                    .with_config(self._config_manager)
                    .build()
                )
                logger.info(f"Publishing message {i} to ipc")
                MessagingClient.publish(self._hello_topic, test_message)
                logger.info(f"Publishing message {i} to iot core")
                try:
                    MessagingClient.publish_northbound(
                        self._hello_topic, test_message, QOS.AT_LEAST_ONCE
                    )
                except Exception as e:
                    logger.warning(f"failed to publish to IoT Core: {e}")
                # Append the data point to the durable telemetry stream (partitioned by Thing).
                if self._stream is not None:
                    thing = self._config_manager.get_thing_name()
                    payload = json.dumps({"msg_id": i, "thing": thing}).encode("utf-8")
                    try:
                        self._stream.append(thing, int(time.time() * 1000), payload)
                    except Exception as e:
                        logger.warning(f"failed to append to telemetry stream: {e}")
                # Use the measure name defined on the metric ("latency"); a mismatch would have the
                # metric target skip the data point (see CloudWatch target's defensive guard).
                measure_values["latency"] = random() * 100
                MetricEmitter.emit_metric("performance", measure_values)

                i += 1
                time.sleep(self._publish_interval)
        except KeyboardInterrupt:
            print("Finished")

    def on_configuration_change(self, configuration) -> bool:
        self._publish_interval = self._config_manager.get_global_config()[
            "publish_interval"
        ]
        return True
