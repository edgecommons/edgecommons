"""Pre-commit component configuration validation.

Candidate validators run after the canonical schema check but before a configuration
generation becomes current.  They receive defensive copies of the candidate and of the
redacted prior generation, so application-specific validation cannot mutate live state or
observe secret-bearing configuration.
"""

from __future__ import annotations

import copy
import queue
import re
import threading
import time
import unicodedata
from dataclasses import dataclass
from enum import Enum
from typing import Callable, Mapping, Optional, Tuple


DEFAULT_CANDIDATE_VALIDATION_TIMEOUT_SECS = 5.0
MAX_CANDIDATE_VALIDATION_TIMEOUT_SECS = 60.0
_MAX_VALIDATOR_THREADS = 4
_MAX_DIAGNOSTIC_CHARS = 256
_CODE = re.compile(r"^[A-Z][A-Z0-9_]{0,63}$")
_IN_VALIDATOR_CALLBACK = threading.local()
_GLOBAL_VALIDATOR_WORKER_PERMITS = threading.BoundedSemaphore(
    _MAX_VALIDATOR_THREADS
)


class ConfigurationValidationPhase(str, Enum):
    """The lifecycle phase of one configuration candidate."""

    INITIAL = "INITIAL"
    RELOAD = "RELOAD"


@dataclass(frozen=True)
class ConfigurationValidationResult:
    """One validator's deterministic accept/reject verdict."""

    accepted: bool
    code: str = ""
    message: str = ""

    def __post_init__(self) -> None:
        if self.accepted:
            object.__setattr__(self, "code", "")
            object.__setattr__(self, "message", "")
            return
        if not _CODE.fullmatch(self.code or ""):
            raise ValueError(
                "configuration validator rejection code must be stable SCREAMING_SNAKE_CASE"
            )
        object.__setattr__(self, "message", self.message or "")

    @staticmethod
    def accept() -> "ConfigurationValidationResult":
        return ConfigurationValidationResult(True)

    @staticmethod
    def reject(code: str, message: str = "") -> "ConfigurationValidationResult":
        return ConfigurationValidationResult(False, code, message)


@dataclass(frozen=True)
class ConfigurationValidationError:
    """A stable, operator-safe pre-commit validator failure."""

    validator: str
    code: str
    message: str


ConfigurationCandidateValidator = Callable[
    [dict, Optional[dict], ConfigurationValidationPhase], ConfigurationValidationResult
]


class ConfigurationCandidateRejected(RuntimeError):
    """Raised when the initial configuration is rejected before provider startup."""

    def __init__(self, errors: Tuple[ConfigurationValidationError, ...]):
        self.errors = tuple(errors)
        detail = "; ".join(
            f"{error.validator}:{error.code}: {error.message}" for error in self.errors
        )
        super().__init__(f"initial configuration rejected: {detail}")


def require_validation_timeout(timeout_secs: float) -> float:
    """Validate and normalize the one-generation overall deadline."""

    if isinstance(timeout_secs, bool) or not isinstance(timeout_secs, (int, float)):
        raise TypeError("configuration validation timeout must be a number of seconds")
    timeout = float(timeout_secs)
    if not 0 < timeout <= MAX_CANDIDATE_VALIDATION_TIMEOUT_SECS:
        raise ValueError(
            "configuration validation timeout must be greater than 0 and no greater than 60 seconds"
        )
    return timeout


def normalize_validators(
    validators: Optional[Mapping[str, ConfigurationCandidateValidator]],
) -> Tuple[Tuple[str, ConfigurationCandidateValidator], ...]:
    """Validate names/callables and retain deterministic registration order."""

    if validators is None:
        return ()
    normalized = []
    for name, validator in validators.items():
        if not isinstance(name, str) or not name.strip():
            raise ValueError("configuration validator name must be a non-empty string")
        if not callable(validator):
            raise TypeError(f"configuration validator '{name}' must be callable")
        normalized.append((name, validator))
    return tuple(normalized)


def in_validator_callback() -> bool:
    return bool(getattr(_IN_VALIDATOR_CALLBACK, "active", False))


def sanitize_validation_message(message: object) -> str:
    """Bound and neutralize a validator-controlled diagnostic."""

    source = "" if message is None else str(message)
    safe = []
    for char in source:
        if len(safe) >= _MAX_DIAGNOSTIC_CHARS:
            break
        safe.append(" " if unicodedata.category(char).startswith("C") else char)
    return " ".join("".join(safe).split())


def validate_candidate(
    validators: Tuple[Tuple[str, ConfigurationCandidateValidator], ...],
    candidate: dict,
    redacted_current: Optional[dict],
    phase: ConfigurationValidationPhase,
    timeout_secs: float,
) -> Tuple[ConfigurationValidationError, ...]:
    """Run validators with at most four daemon workers and one overall deadline.

    Python cannot safely kill an arbitrary callback.  Daemon workers let the bounded caller
    return at the deadline; every callback owns private copies, so a tardy validator cannot
    mutate either the accepted snapshot or another validator's view.
    """

    if not validators:
        return ()

    tasks: queue.Queue = queue.Queue()
    results: queue.Queue = queue.Queue()
    for index, named in enumerate(validators):
        tasks.put((index, named))

    def worker() -> None:
        try:
            while True:
                try:
                    index, (name, validator) = tasks.get_nowait()
                except queue.Empty:
                    return
                try:
                    _IN_VALIDATOR_CALLBACK.active = True
                    result = validator(
                        copy.deepcopy(candidate),
                        None
                        if redacted_current is None
                        else copy.deepcopy(redacted_current),
                        phase,
                    )
                    if result is None:
                        error = ConfigurationValidationError(
                            name, "VALIDATOR_FAILED", "validator returned no result"
                        )
                    elif not isinstance(result, ConfigurationValidationResult):
                        error = ConfigurationValidationError(
                            name,
                            "VALIDATOR_FAILED",
                            "validator returned an unsupported result",
                        )
                    elif result.accepted:
                        error = None
                    else:
                        error = ConfigurationValidationError(
                            name, result.code, sanitize_validation_message(result.message)
                        )
                except Exception as exc:  # noqa: BLE001 - failures are stable rejections
                    error = ConfigurationValidationError(
                        name, "VALIDATOR_FAILED", sanitize_validation_message(exc)
                    )
                finally:
                    _IN_VALIDATOR_CALLBACK.active = False
                    tasks.task_done()
                results.put((index, error))
        finally:
            # A timed-out callback may outlive its caller.  Retain this process-wide
            # permit until the daemon actually exits so repeated reloads cannot create
            # an unbounded number of surviving validator threads.
            _GLOBAL_VALIDATOR_WORKER_PERMITS.release()

    for worker_index in range(min(_MAX_VALIDATOR_THREADS, len(validators))):
        if not _GLOBAL_VALIDATOR_WORKER_PERMITS.acquire(blocking=False):
            break
        thread = threading.Thread(
            target=worker,
            name=f"edgecommons-config-validator-{worker_index + 1}",
            daemon=True,
        )
        try:
            thread.start()
        except BaseException:
            _GLOBAL_VALIDATOR_WORKER_PERMITS.release()
            raise

    deadline = time.monotonic() + timeout_secs
    completed = {}
    while len(completed) < len(validators):
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            break
        try:
            index, error = results.get(timeout=remaining)
        except queue.Empty:
            break
        completed[index] = error

    errors = []
    for index, (name, _validator) in enumerate(validators):
        if index not in completed:
            errors.append(
                ConfigurationValidationError(
                    name,
                    "VALIDATION_TIMEOUT",
                    "configuration validation exceeded its bounded deadline",
                )
            )
        elif completed[index] is not None:
            errors.append(completed[index])
    return tuple(errors)
