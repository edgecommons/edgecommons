"""
General utility functions for ggcommons.

This module provides common utility functions used throughout the ggcommons library.
"""

import logging
import time
import threading
from datetime import datetime, timezone
from typing import Any, Dict, Optional, Union, List
import json
import os

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
    def sleep(milliseconds: Union[int, float]) -> None:
        """
        Thread sleep utility that handles InterruptedException.
        
        Args:
            milliseconds: Sleep duration in milliseconds
        """
        if milliseconds <= 0:
            return
            
        try:
            time.sleep(milliseconds / 1000.0)
        except KeyboardInterrupt:
            logger.debug("Sleep interrupted")
            raise
            
    @staticmethod
    def safe_json_loads(json_str: str, default: Any = None) -> Any:
        """
        Safely parse JSON string with error handling.
        
        Args:
            json_str: JSON string to parse
            default: Default value to return on error
            
        Returns:
            Parsed JSON object or default value
        """
        if not json_str:
            return default
            
        try:
            return json.loads(json_str)
        except (json.JSONDecodeError, TypeError) as e:
            logger.warning(f"Failed to parse JSON: {e}")
            return default
            
    @staticmethod
    def safe_json_dumps(obj: Any, default: str = "{}") -> str:
        """
        Safely serialize object to JSON string.
        
        Args:
            obj: Object to serialize
            default: Default value to return on error
            
        Returns:
            JSON string or default value
        """
        try:
            return json.dumps(obj, indent=2, default=str)
        except (TypeError, ValueError) as e:
            logger.warning(f"Failed to serialize to JSON: {e}")
            return default
            
    @staticmethod
    def ensure_directory_exists(file_path: str) -> None:
        """
        Ensure the directory for a file path exists.
        
        Args:
            file_path: File path to check
            
        Raises:
            OSError: If directory cannot be created
        """
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
    def read_file_safe(file_path: str, encoding: str = 'utf-8') -> Optional[str]:
        """
        Safely read file contents with error handling.
        
        Args:
            file_path: Path to file to read
            encoding: File encoding
            
        Returns:
            File contents or None on error
        """
        if not file_path or not os.path.exists(file_path):
            return None
            
        try:
            with open(file_path, 'r', encoding=encoding) as f:
                return f.read()
        except (IOError, OSError, UnicodeDecodeError) as e:
            logger.error(f"Failed to read file {file_path}: {e}")
            return None
            
    @staticmethod
    def write_file_safe(file_path: str, content: str, encoding: str = 'utf-8') -> bool:
        """
        Safely write content to file with error handling.
        
        Args:
            file_path: Path to file to write
            content: Content to write
            encoding: File encoding
            
        Returns:
            True if successful, False otherwise
        """
        if not file_path or content is None:
            return False
            
        try:
            # Ensure directory exists
            Utils.ensure_directory_exists(file_path)
            
            with open(file_path, 'w', encoding=encoding) as f:
                f.write(content)
            return True
        except (IOError, OSError, UnicodeEncodeError) as e:
            logger.error(f"Failed to write file {file_path}: {e}")
            return False
            
    @staticmethod
    def get_file_size(file_path: str) -> int:
        """
        Get file size in bytes.
        
        Args:
            file_path: Path to file
            
        Returns:
            File size in bytes, or 0 if file doesn't exist
        """
        try:
            return os.path.getsize(file_path) if os.path.exists(file_path) else 0
        except OSError:
            return 0
            
    @staticmethod
    def is_file_readable(file_path: str) -> bool:
        """
        Check if file exists and is readable.
        
        Args:
            file_path: Path to file
            
        Returns:
            True if file is readable
        """
        return os.path.exists(file_path) and os.access(file_path, os.R_OK)
        
    @staticmethod
    def is_file_writable(file_path: str) -> bool:
        """
        Check if file is writable (or directory is writable for new files).
        
        Args:
            file_path: Path to file
            
        Returns:
            True if file is writable
        """
        if os.path.exists(file_path):
            return os.access(file_path, os.W_OK)
        else:
            # Check if directory is writable
            directory = os.path.dirname(file_path)
            return os.access(directory, os.W_OK) if directory else False
            
    @staticmethod
    def merge_dicts(dict1: Dict[str, Any], dict2: Dict[str, Any], 
                   deep_merge: bool = True) -> Dict[str, Any]:
        """
        Merge two dictionaries with optional deep merging.
        
        Args:
            dict1: First dictionary
            dict2: Second dictionary (takes precedence)
            deep_merge: Whether to perform deep merge of nested dicts
            
        Returns:
            Merged dictionary
        """
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
                result[key] = Utils.merge_dicts(result[key], value, deep_merge)
            else:
                result[key] = value
                
        return result
        
    @staticmethod
    def flatten_dict(d: Dict[str, Any], parent_key: str = '', 
                    separator: str = '.') -> Dict[str, Any]:
        """
        Flatten a nested dictionary.
        
        Args:
            d: Dictionary to flatten
            parent_key: Parent key prefix
            separator: Key separator
            
        Returns:
            Flattened dictionary
        """
        items = []
        
        for key, value in d.items():
            new_key = f"{parent_key}{separator}{key}" if parent_key else key
            
            if isinstance(value, dict):
                items.extend(Utils.flatten_dict(value, new_key, separator).items())
            else:
                items.append((new_key, value))
                
        return dict(items)
        
    @staticmethod
    def get_nested_value(d: Dict[str, Any], key_path: str, 
                        separator: str = '.', default: Any = None) -> Any:
        """
        Get value from nested dictionary using dot notation.
        
        Args:
            d: Dictionary to search
            key_path: Dot-separated key path
            separator: Key separator
            default: Default value if key not found
            
        Returns:
            Value at key path or default
        """
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
    def set_nested_value(d: Dict[str, Any], key_path: str, value: Any,
                        separator: str = '.') -> None:
        """
        Set value in nested dictionary using dot notation.
        
        Args:
            d: Dictionary to modify
            key_path: Dot-separated key path
            value: Value to set
            separator: Key separator
        """
        if not d or not key_path:
            return
            
        keys = key_path.split(separator)
        current = d
        
        # Navigate to parent of target key
        for key in keys[:-1]:
            if key not in current:
                current[key] = {}
            current = current[key]
            
        # Set the final value
        current[keys[-1]] = value
        
    @staticmethod
    def validate_required_keys(d: Dict[str, Any], required_keys: List[str]) -> List[str]:
        """
        Validate that dictionary contains required keys.
        
        Args:
            d: Dictionary to validate
            required_keys: List of required keys
            
        Returns:
            List of missing keys
        """
        if not d:
            return required_keys.copy()
            
        missing_keys = []
        for key in required_keys:
            if key not in d:
                missing_keys.append(key)
                
        return missing_keys
        
    @staticmethod
    def sanitize_filename(filename: str) -> str:
        """
        Sanitize filename by removing invalid characters.
        
        Args:
            filename: Original filename
            
        Returns:
            Sanitized filename
        """
        if not filename:
            return "unnamed"
            
        # Remove invalid characters
        invalid_chars = '<>:"/\\|?*'
        sanitized = ''.join(c for c in filename if c not in invalid_chars)
        
        # Remove leading/trailing whitespace and dots
        sanitized = sanitized.strip(' .')
        
        # Ensure not empty
        return sanitized if sanitized else "unnamed"
        
    @staticmethod
    def format_bytes(bytes_value: int) -> str:
        """
        Format bytes value as human-readable string.
        
        Args:
            bytes_value: Number of bytes
            
        Returns:
            Formatted string (e.g., "1.5 MB")
        """
        if bytes_value < 1024:
            return f"{bytes_value} B"
        elif bytes_value < 1024 ** 2:
            return f"{bytes_value / 1024:.1f} KB"
        elif bytes_value < 1024 ** 3:
            return f"{bytes_value / (1024 ** 2):.1f} MB"
        else:
            return f"{bytes_value / (1024 ** 3):.1f} GB"
            
    @staticmethod
    def format_duration(seconds: float) -> str:
        """
        Format duration in seconds as human-readable string.
        
        Args:
            seconds: Duration in seconds
            
        Returns:
            Formatted string (e.g., "1h 30m 45s")
        """
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
    """
    
    def __init__(self, initial_value: int = 0):
        """
        Initialize counter.
        
        Args:
            initial_value: Initial counter value
        """
        self._value = initial_value
        self._lock = threading.Lock()
        
    def increment(self, amount: int = 1) -> int:
        """
        Increment counter and return new value.
        
        Args:
            amount: Amount to increment
            
        Returns:
            New counter value
        """
        with self._lock:
            self._value += amount
            return self._value
            
    def decrement(self, amount: int = 1) -> int:
        """
        Decrement counter and return new value.
        
        Args:
            amount: Amount to decrement
            
        Returns:
            New counter value
        """
        with self._lock:
            self._value -= amount
            return self._value
            
    def get(self) -> int:
        """
        Get current counter value.
        
        Returns:
            Current counter value
        """
        with self._lock:
            return self._value
            
    def set(self, value: int) -> None:
        """
        Set counter value.
        
        Args:
            value: New counter value
        """
        with self._lock:
            self._value = value
            
    def reset(self) -> int:
        """
        Reset counter to zero and return previous value.
        
        Returns:
            Previous counter value
        """
        with self._lock:
            old_value = self._value
            self._value = 0
            return old_value