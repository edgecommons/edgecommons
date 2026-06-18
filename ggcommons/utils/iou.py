import logging
from threading import Event
from typing import Any, Tuple

logger = logging.getLogger("iou")


class Iou:
    """A one-shot "I owe you" future for request/reply.

    Backed by a threading.Event (rather than an acquired Lock) so a duplicate
    set_result() is harmless and the wait has clean timeout semantics. Public
    contract is unchanged:
      - get(timeout) -> (done, result); on timeout returns (False, self),
        otherwise (True, result). timeout < 0 blocks indefinitely.
      - set_result(result) completes the future.
      - done() reports completion; get_user_data() returns the supplied data.
    """

    def __init__(self, user_data: Any = None):
        self._event = Event()
        self._result = None
        self._user_data = user_data

    def get(self, timeout: float = -1) -> Tuple[bool, Any]:
        # timeout < 0 means "wait forever" (Event.wait(None) blocks indefinitely).
        wait_timeout = None if timeout is not None and timeout < 0 else timeout
        self._event.wait(wait_timeout)
        if not self._event.is_set():
            return False, self
        return True, self._result

    def set_result(self, result: Any):
        self._result = result
        self._event.set()

    def done(self) -> bool:
        return self._event.is_set()

    def get_user_data(self) -> Any:
        return self._user_data
