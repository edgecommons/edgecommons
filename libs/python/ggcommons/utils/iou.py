import logging
import threading
from threading import Event
from typing import Any, Optional, Tuple

logger = logging.getLogger("iou")


class Iou:
    """A one-shot "I owe you" future for request/reply.

    Backed by a threading.Event (rather than an acquired Lock) so a duplicate
    set_result() is harmless and the wait has clean timeout semantics. Public
    contract:
      - get(timeout) -> (done, result); on timeout returns (False, self),
        otherwise (True, result). timeout < 0 blocks indefinitely.
        **Raises** the stored error (a RequestTimeoutError when the framework-owned
        request deadline fired — UNS-CANONICAL-DESIGN §5, D-U23) when the request was
        completed exceptionally.
      - set_result(result) completes the future; set_error(exc) completes it
        exceptionally.
      - done() reports completion; get_user_data() returns the supplied data.

    The request's single idempotent *settle* path (§5.1): reply-arrival, the framework
    deadline and cancel_request all race through :meth:`try_settle`; exactly one wins
    and performs the cleanup (unsubscribe + pending-entry removal) and the completion;
    the losers no-op.
    """

    def __init__(self, user_data: Any = None):
        self._event = Event()
        self._result = None
        self._error: Optional[BaseException] = None
        self._user_data = user_data
        self._settled = False
        self._settle_lock = threading.Lock()
        # The framework-owned deadline timer, when one was armed; canceled by the
        # settle winner so a settled request never fires a stale deadline.
        self._deadline_timer: Optional[threading.Timer] = None

    def try_settle(self) -> bool:
        """Attempts to settle this request. Exactly one of reply-arrival, the framework
        deadline and cancel_request wins; the winner owns the cleanup (unsubscribe the
        reply topic, remove the pending entry) and the completion of this future. The
        winner also cancels the armed deadline timer (if any).

        :returns: ``True`` for the settle winner; ``False`` when the request was
            already settled (the caller must no-op)
        """
        with self._settle_lock:
            if self._settled:
                return False
            self._settled = True
            timer = self._deadline_timer
        if timer is not None:
            timer.cancel()
        return True

    def is_settled(self) -> bool:
        """Whether this request has been settled (by reply-arrival, deadline or cancel)."""
        with self._settle_lock:
            return self._settled

    def _set_deadline_timer(self, timer: threading.Timer) -> None:
        """Attaches the framework-owned deadline timer for this request so the settle
        winner can cancel it. If the request was already settled by the time the timer
        is attached (a reply can beat the arming call), the timer is canceled
        immediately."""
        with self._settle_lock:
            self._deadline_timer = timer
            settled = self._settled
        if settled and timer is not None:
            timer.cancel()

    def get(self, timeout: float = -1) -> Tuple[bool, Any]:
        # timeout < 0 means "wait forever" (Event.wait(None) blocks indefinitely).
        wait_timeout = None if timeout is not None and timeout < 0 else timeout
        self._event.wait(wait_timeout)
        if not self._event.is_set():
            return False, self
        if self._error is not None:
            raise self._error
        return True, self._result

    def set_result(self, result: Any):
        self._result = result
        self._event.set()

    def set_error(self, error: BaseException):
        """Completes this future exceptionally: a waiting (or later) :meth:`get`
        raises ``error`` instead of returning a result."""
        self._error = error
        self._event.set()

    def done(self) -> bool:
        return self._event.is_set()

    def get_user_data(self) -> Any:
        return self._user_data
