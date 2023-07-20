from threading import Lock
import logging
from typing import Any, Tuple

logger = logging.getLogger("iou")


class Iou:

    def __init__(self):
        self._lock = Lock()
        self._lock.acquire()
        self._result = None
        self._done = False

    def get(self, timeout: float = -1) -> Tuple[bool, Any]:
        self._lock.acquire(timeout=timeout)
        return self._done, self._result

    def set_result(self, result: Any):
        self._result = result
        self._done = True
        self._lock.release()

    def done(self) -> bool:
        return self._done
