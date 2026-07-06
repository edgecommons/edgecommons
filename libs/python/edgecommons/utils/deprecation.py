"""Helpers for marking deprecated public API.

These mark surfaces scheduled for removal in a future release. They emit a
DeprecationWarning at call/construction time so component authors get a warning
before the API is deleted.
"""
import functools
import warnings


def deprecated(message):
    """Decorator: emit a DeprecationWarning when the wrapped callable is invoked."""
    def decorator(func):
        @functools.wraps(func)
        def wrapper(*args, **kwargs):
            warnings.warn(message, DeprecationWarning, stacklevel=2)
            return func(*args, **kwargs)
        return wrapper
    return decorator


def warn_deprecated(message):
    """Emit a DeprecationWarning directly (e.g. from a class __init__)."""
    warnings.warn(message, DeprecationWarning, stacklevel=3)
