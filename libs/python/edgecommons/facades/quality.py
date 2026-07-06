"""The normalized, protocol-independent sample-quality verdict of the southbound contract
(DESIGN-class-facades §2.1, ``docs/SOUTHBOUND.md`` §3). The wire token is the enum's
**UPPERCASE** value -- ``GOOD | BAD | UNCERTAIN`` -- carried verbatim on every ``data``
sample.

:class:`~edgecommons.facades.data_facade.DataFacade` defaults an omitted sample quality to
:attr:`Quality.GOOD` (marking the synthesis with ``qualityRaw: "unspecified"``), so a
sample can never reach the bus without a quality -- the structural guarantee the facade
exists to make.

Mirrors Java's ``Quality`` enum (``com.mbreissi.edgecommons.facades.Quality``).
"""
from enum import Enum
from typing import Optional


class Quality(str, Enum):
    """The three-value sample-quality verdict. A ``str`` subclass so a ``Quality`` member
    serializes as its own wire token when placed straight into a JSON-bound ``dict``."""

    #: The value is trustworthy (the default for a sample carrying a value with no verdict).
    GOOD = "GOOD"
    #: The value is not trustworthy (exception/timeout/failed read).
    BAD = "BAD"
    #: The value is present but suspect (stale/partial).
    UNCERTAIN = "UNCERTAIN"

    def wire(self) -> str:
        """The wire token -- the UPPERCASE value exactly as it appears in a ``data`` sample."""
        return self.value

    @staticmethod
    def from_wire(token: Optional[str]) -> Optional["Quality"]:
        """Resolves a wire token to its quality, or ``None`` when outside the closed set."""
        try:
            return Quality(token)
        except ValueError:
            return None
