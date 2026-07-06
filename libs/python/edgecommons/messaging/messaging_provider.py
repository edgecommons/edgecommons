import abc
import logging
import threading
from abc import abstractmethod
from typing import Callable, Optional
from edgecommons.messaging.errors import RequestTimeoutError
from edgecommons.messaging.message import Message
from edgecommons.utils.iou import Iou
from awsiot.greengrasscoreipc.model import QOS

# Default per-subscription queue bound (drop on overflow) when a caller doesn't specify one.
DEFAULT_MAX_MESSAGES = 10000

# The built-in request() deadline (seconds) that applies until the config-model default
# (messaging.requestTimeoutSeconds) is late-bound after ConfigManager construction
# (UNS-CANONICAL-DESIGN §5 / D-U5). Deliberately non-zero so the CONFIG_COMPONENT
# bootstrap request gets a deadline instead of hanging forever.
DEFAULT_REQUEST_TIMEOUT_SECONDS = 30.0

_logger = logging.getLogger("MessagingProvider")


class MessagingProvider(metaclass=abc.ABCMeta):
    # Class-level default so instances constructed via __new__ (test seams) still carry
    # the built-in deadline; set_default_request_timeout shadows it per instance.
    _default_request_timeout: float = DEFAULT_REQUEST_TIMEOUT_SECONDS

    def __init__(self):
        pass

    # ----- framework-owned request() deadline (UNS-CANONICAL-DESIGN §5) ----------------

    def set_default_request_timeout(self, timeout_secs: Optional[float]) -> None:
        """Sets the default ``request()`` deadline (the late-bind hook for
        ``messaging.requestTimeoutSeconds``, §5/D-U5). ``None`` or a zero/negative value
        disables the default deadline; an explicit per-call timeout always wins over
        this default."""
        self._default_request_timeout = (
            float(timeout_secs) if timeout_secs and timeout_secs > 0 else 0.0
        )

    def get_default_request_timeout(self) -> float:
        """The current default ``request()`` deadline in seconds (``0`` = disabled)."""
        return self._default_request_timeout

    def _effective_request_timeout(self, per_call: Optional[float]) -> Optional[float]:
        """Resolves the deadline for one ``request()`` call: an explicit per-call
        timeout wins (including ``0`` = disabled for that call); ``None`` falls back to
        the provider default. A zero/negative resolution yields ``None`` (no
        deadline)."""
        chosen = per_call if per_call is not None else self._default_request_timeout
        if chosen is None or chosen <= 0:
            return None
        return float(chosen)

    def _arm_request_deadline(self, iou: Iou, timeout_secs: Optional[float],
                              cleanup: Callable[[], None]) -> None:
        """Arms the framework-owned deadline timer for a request at send time (§5).
        When the deadline fires and wins the request's settle CAS
        (:meth:`~edgecommons.utils.iou.Iou.try_settle`), it (1) runs the provider-supplied
        cleanup (unsubscribe the ephemeral reply topic, remove the pending entry) and
        (2) completes the future **exceptionally** with a
        :class:`~edgecommons.messaging.errors.RequestTimeoutError` — even if the caller
        never ``get()``'s the future (the reply-subscription leak fix). No-op when
        ``timeout_secs`` is ``None`` (deadline disabled)."""
        if timeout_secs is None:
            return

        def _on_deadline():
            if not iou.try_settle():
                return  # reply or cancel won the settle race — the deadline no-ops
            try:
                cleanup()
            except Exception as e:  # noqa: BLE001 - cleanup must not mask the timeout
                _logger.warning(
                    f"Request-deadline cleanup for reply topic '{iou.get_user_data()}'"
                    f" failed: {e}"
                )
            iou.set_error(RequestTimeoutError(
                f"request timed out after {timeout_secs} s waiting for a reply on"
                f" '{iou.get_user_data()}'"
            ))

        timer = threading.Timer(timeout_secs, _on_deadline)
        timer.daemon = True
        timer.start()
        iou._set_deadline_timer(timer)

    @abstractmethod
    def disconnect(self):
        pass

    @abstractmethod
    def connected(self) -> bool:
        """Whether the provider currently has a usable broker/IPC connection.

        Backs the readiness probe (FR-HB-1): ``/readyz`` is 200 only when this is ``True`` (and the
        component is ready and not shutting down). It MUST be cheap/non-blocking — it is queried on
        every readiness check. Liveness deliberately does NOT consult it (a broker outage must not
        fail ``/livez``).
        """
        pass

    @abstractmethod
    def publish(self, topic: str, msg: Message):
        pass

    @abstractmethod
    def publish_raw(self, topic: str, msg: dict):
        pass

    @abstractmethod
    def publish_to_iot_core(self, topic: str, msg: Message, qos: str):
        pass

    @abstractmethod
    def publish_to_iot_core_raw(self, topic: str, msg: dict, qos: str):
        pass

    @abstractmethod
    def subscribe(
        self,
        topic: str,
        callback: Callable[[str, Message], None],
        max_concurrency: int = None,
        max_messages: int = None,
    ):
        pass

    @abstractmethod
    def subscribe_to_iot_core(
        self,
        topic: str,
        callback: Callable[[str, Message], None],
        qos: QOS,
        max_concurrency: int = None,
        max_messages: int = None,
    ):
        pass

    @abstractmethod
    def unsubscribe(self, topic: str):
        pass

    @abstractmethod
    def unsubscribe_from_iot_core(self, topic: str):
        pass

    @abstractmethod
    def request(self, topic: str, msg: Message, timeout_secs: Optional[float] = None) -> Iou:
        """Sends a request; the returned Iou carries the framework-owned deadline
        (UNS-CANONICAL-DESIGN §5): ``None`` uses the configured default
        (``messaging.requestTimeoutSeconds``), ``0`` disables the deadline for this
        call, an explicit value always wins over the default."""
        pass

    @abstractmethod
    def request_from_iot_core(self, topic: str, msg: Message,
                              timeout_secs: Optional[float] = None) -> Iou:
        """IoT Core variant of :meth:`request` (same deadline semantics)."""
        pass

    @abstractmethod
    def reply(self, request_msg: Message, response_msg: Message):
        pass

    @abstractmethod
    def reply_to_iot_core(self, request_msg: Message, response_msg: Message):
        pass

    @abstractmethod
    def cancel_request(self, iou: Iou):
        pass

    @abstractmethod
    def cancel_request_from_iot_core(self, iou: Iou):
        pass

    @abstractmethod
    def get_native_client(self):
        pass

    # Copied from open source Paho MQTT python client
    # (https://github.com/thejuan/paho-mqtt-python/blob/master/src/paho/mqtt/client.py)
    # Under the Eclipse Public License (https://github.com/thejuan/paho-mqtt-python/blob/master/LICENSE.txt)
    @staticmethod
    def topic_matches_sub(sub: str, topic: str) -> bool:
        """Check whether a topic matches a subscription.
        For example:
        foo/bar would match the subscription foo/# or +/bar
        non/matching would not match the subscription non/+/+
        """
        result = True
        multilevel_wildcard = False

        slen = len(sub)
        tlen = len(topic)

        if slen > 0 and tlen > 0:
            if (sub[0] == "$" and topic[0] != "$") or (
                topic[0] == "$" and sub[0] != "$"
            ):
                return False

        spos = 0
        tpos = 0

        while spos < slen and tpos < tlen:
            if sub[spos] == topic[tpos]:
                if tpos == tlen - 1:
                    # Check for e.g. foo matching foo/#
                    if (
                        spos == slen - 3
                        and sub[spos + 1] == "/"
                        and sub[spos + 2] == "#"
                    ):
                        result = True
                        multilevel_wildcard = True
                        break

                spos += 1
                tpos += 1

                if tpos == tlen and spos == slen - 1 and sub[spos] == "+":
                    spos += 1
                    result = True
                    break
            else:
                if sub[spos] == "+":
                    spos += 1
                    while tpos < tlen and topic[tpos] != "/":
                        tpos += 1
                    if tpos == tlen and spos == slen:
                        result = True
                        break

                elif sub[spos] == "#":
                    multilevel_wildcard = True
                    if spos + 1 != slen:
                        result = False
                        break
                    else:
                        result = True
                        break

                else:
                    result = False
                    break

        if not multilevel_wildcard and (tpos < tlen or spos < slen):
            result = False

        return result
