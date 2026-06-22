"""Parameter subsystem error type."""


class ParameterError(Exception):
    """Any parameter source/cache failure (unreachable backend, bad config, I/O, parse error).

    Messages never include a ``secure`` parameter's value.
    """
