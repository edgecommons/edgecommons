"""
General utility functions for ggcommons.

This module provides common utility functions used throughout the ggcommons library.

Deprecation note: only ``Utils.get_utc_z()`` is used by the library. The remaining
``Utils`` helpers and ``ThreadSafeCounter`` are unused and are deprecated; they emit
a DeprecationWarning and are scheduled for removal in a future release.
"""

import logging
import time
import threading
from datetime import datetime, timezone
from typing import Any, Dict, Optional, Union, List
import json
import os

from ggcommons.utils.deprecation import deprecated, warn_deprecated

logger = logging.getLogger(__name__)


class Utils:
    """
    Collection of general utility methods.
    """

    @staticmethod
    def get_utc_z() -> str:
        """
        Get current UTC time in ISO 8601 format.

        Returns:
            Current UTC time in ISO 8601 format
        """
        return datetime.now(timezone.utc).isoformat(timespec='milliseconds').replace('+00:00', 'Z')

    @staticmethod
    @deprecated("Utils.sleep is deprecated and will be removed in a future release.")
    def sleep(milliseconds: Union[int, float]) -> None:
        """Thread sleep utility. Deprecated; use time.sleep directly."""
        if milliseconds <= 0:
            return

        try:
            time.sleep(milliseconds / 1000.0)
        except KeyboardInterrupt:
            logger.debug("Sleep interrupted")
            raise

    @staticmethod
    @deprecated("Utils.safe_json_loads is deprecated and will be removed in a future release.")
    def safe_json_loads(json_str: str, default: Any = None) -> Any:
        """Safely parse JSON string with error handling. Deprecated."""
        if not json_str:
            return default

        try:
            return json.loads(json_str)
        except (json.JSONDecodeError, TypeError) as e:
            logger.warning(f"Failed to parse JSON: {e}")
            return default

    @staticmethod
    @deprecated("Utils.safe_json_dumps is deprecated and will be removed in a future release.")
    def safe_json_dumps(obj: Any, default: str = "{}") -> str:
        """Safely serialize object to JSON string. Deprecated."""
        try:
            return json.dumps(obj, indent=2, default=str)
        except (TypeError, ValueError) as e:
            logger.warning(f"Failed to serialize to JSON: {e}")
            return default

    @staticmethod
    @deprecated("Utils.ensure_directory_exists is deprecated and will be removed in a future release.")
    def ensure_directory_exists(file_path: str) -> None:
        """Ensure the directory for a file path exists. Deprecated."""
        if not file_path:
            return

        directory = os.path.dirname(file_path)
        if directory and not os.path.exists(directory):
            try:
                os.makedirs(directory, exist_ok=True)
                logger.debug(f"Created directory: {directory}")
            except OSError as e:
                logger.error(f"Failed to create directory {directory}: {e}")
                raise

    @staticmethod
    @deprecated("Utils.read_file_safe is deprecated and will be removed in a future release.")
    def read_file_safe(file_path: str, encoding: str = 'utf-8') -> Optional[str]:
        """Safely read file contents with error handling. Deprecated."""
        if not file_path or not os.path.exists(file_path):
            return None

        try:
            with open(file_path, 'r', encoding=encoding) as f:
                return f.read()
        except (IOError, OSError, UnicodeDecodeError) as e:
            logger.error(f"Failed to read file {file_path}: {e}")
            return None

    @staticmethod
    @deprecated("Utils.write_file_safe is deprecated and will be removed in a future release.")
    def write_file_safe(file_path: str, content: str, encoding: str = 'utf-8') -> bool:
        """Safely write content to file with error handling. Deprecated."""
        if not file_path or content is None:
            return False

        try:
            directory = os.path.dirname(file_path)
            if directory and not os.path.exists(directory):
                os.makedirs(directory, exist_ok=True)

            with open(file_path, 'w', encoding=encoding) as f:
                f.write(content)
            return True
        except (IOError, OSError, UnicodeEncodeError) as e:
            logger.error(f"Failed to write file {file_path}: {e}")
            return False

    @staticmethod
    @deprecated("Utils.get_file_size is deprecated and will be removed in a future release.")
    def get_file_size(file_path: str) -> int:
        """Get file size in bytes. Deprecated."""
        try:
            return os.path.getsize(file_path) if os.path.exists(file_path) else 0
        except OSError:
            return 0

    @staticmethod
    @deprecated("Utils.is_file_readable is deprecated and will be removed in a future release.")
    def is_file_readable(file_path: str) -> bool:
        """Check if file exists and is readable. Deprecated."""
        return os.path.exists(file_path) and os.access(file_path, os.R_OK)

    @staticmethod
    @deprecated("Utils.is_file_writable is deprecated and will be removed in a future release.")
    def is_file_writable(file_path: str) -> bool:
        """Check if file is writable. Deprecated."""
        if os.path.exists(file_path):
            return os.access(file_path, os.W_OK)
        else:
            directory = os.path.dirname(file_path)
            return os.access(directory, os.W_OK) if directory else False

    @staticmethod
    @deprecated("Utils.merge_dicts is deprecated and will be removed in a future release.")
    def merge_dicts(dict1: Dict[str, Any], dict2: Dict[str, Any],
                    deep_merge: bool = True) -> Dict[str, Any]:
        """Merge two dictionaries with optional deep merging. Deprecated."""
        if not dict1:
            return dict2.copy() if dict2 else {}
        if not dict2:
            return dict1.copy()

        result = dict1.copy()

        for key, value in dict2.items():
            if (deep_merge and
                    key in result and
                    isinstance(result[key], dict) and
                    isinstance(value, dict)):
                # Recurse on the underlying function to avoid a nested warning.
                result[key] = Utils.merge_dicts.__wrapped__(result[key], value, deep_merge)
            else:
                result[key] = value

        return result

    @staticmethod
    @deprecated("Utils.flatten_dict is deprecated and will be removed in a future release.")
    def flatten_dict(d: Dict[str, Any], parent_key: str = '',
                     separator: str = '.') -> Dict[str, Any]:
        """Flatten a nested dictionary. Deprecated."""
        items = []

        for key, value in d.items():
            new_key = f"{parent_key}{separator}{key}" if parent_key else key

            if isinstance(value, dict):
                items.extend(Utils.flatten_dict.__wrapped__(value, new_key, separator).items())
            else:
                items.append((new_key, value))

        return dict(items)

    @staticmethod
    @deprecated("Utils.get_nested_value is deprecated and will be removed in a future release.")
    def get_nested_value(d: Dict[str, Any], key_path: str,
                         separator: str = '.', default: Any = None) -> Any:
        """Get value from nested dictionary using dot notation. Deprecated."""
        if not d or not key_path:
            return default

        keys = key_path.split(separator)
        current = d

        try:
            for key in keys:
                current = current[key]
            return current
        except (KeyError, TypeError):
            return default

    @staticmethod
    @deprecated("Utils.set_nested_value is deprecated and will be removed in a future release.")
    def set_nested_value(d: Dict[str, Any], key_path: str, value: Any,
                         separator: str = '.') -> None:
        """Set value in nested dictionary using dot notation. Deprecated."""
        if not d or not key_path:
            return

        keys = key_path.split(separator)
        current = d

        for key in keys[:-1]:
            if key not in current:
                current[key] = {}
            current = current[key]

        current[keys[-1]] = value

    @staticmethod
    @deprecated("Utils.validate_required_keys is deprecated and will be removed in a future release.")
    def validate_required_keys(d: Dict[str, Any], required_keys: List[str]) -> List[str]:
        """Validate that dictionary contains required keys. Deprecated."""
        if not d:
            return required_keys.copy()

        return [key for key in required_keys if key not in d]

    @staticmethod
    @deprecated("Utils.sanitize_filename is deprecated and will be removed in a future release.")
    def sanitize_filename(filename: str) -> str:
        """Sanitize filename by removing invalid characters. Deprecated."""
        if not filename:
            return "unnamed"

        invalid_chars = '<>:"/\\|?*'
        sanitized = ''.join(c for c in filename if c not in invalid_chars)
        sanitized = sanitized.strip(' .')
        return sanitized if sanitized else "unnamed"

    @staticmethod
    @deprecated("Utils.format_bytes is deprecated and will be removed in a future release.")
    def format_bytes(bytes_value: int) -> str:
        """Format bytes value as human-readable string. Deprecated."""
        if bytes_value < 1024:
            return f"{bytes_value} B"
        elif bytes_value < 1024 ** 2:
            return f"{bytes_value / 1024:.1f} KB"
        elif bytes_value < 1024 ** 3:
            return f"{bytes_value / (1024 ** 2):.1f} MB"
        else:
            return f"{bytes_value / (1024 ** 3):.1f} GB"

    @staticmethod
    @deprecated("Utils.format_duration is deprecated and will be removed in a future release.")
    def format_duration(seconds: float) -> str:
        """Format duration in seconds as human-readable string. Deprecated."""
        if seconds < 60:
            return f"{seconds:.1f}s"
        elif seconds < 3600:
            minutes = int(seconds // 60)
            secs = seconds % 60
            return f"{minutes}m {secs:.0f}s"
        else:
            hours = int(seconds // 3600)
            minutes = int((seconds % 3600) // 60)
            secs = seconds % 60
            return f"{hours}h {minutes}m {secs:.0f}s"


class ThreadSafeCounter:
    """
    Thread-safe counter utility.

    Deprecated: unused by the library; scheduled for removal in a future release.
    """

    def __init__(self, initial_value: int = 0):
        warn_deprecated(
            "ThreadSafeCounter is deprecated and will be removed in a future release."
        )
        self._value = initial_value
        self._lock = threading.Lock()

    def increment(self, amount: int = 1) -> int:
        with self._lock:
            self._value += amount
            return self._value

    def decrement(self, amount: int = 1) -> int:
        with self._lock:
            self._value -= amount
            return self._value

    def get(self) -> int:
        with self._lock:
            return self._value

    def set(self, value: int) -> None:
        with self._lock:
            self._value = value

    def reset(self) -> int:
        with self._lock:
            old_value = self._value
            self._value = 0
            return old_value
