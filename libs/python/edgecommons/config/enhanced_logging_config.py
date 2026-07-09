"""
Enhanced logging configuration with advanced features.

This module provides enhanced logging configuration with support for:
- File logging with rotation
- A structured **stdout-JSON sink** (one JSON object per line) — Phase 1c, FR-LOG-1
- Per-logger level configuration
- Global logging control
- Dynamic reconfiguration

The stdout-JSON sink is selected via the existing per-language ``logging.python_format``
token (FR-LOG-4): the case-insensitive value :data:`~edgecommons.platform.resolver.LOGGING_FORMAT_JSON`
(``"json"``) swaps the console handler's *layout* to :class:`JsonLogFormatter` while keeping the
same stdout appender. It is the default on the ``KUBERNETES`` platform (FR-LOG-1) via the
platform-profile default; the effective format follows the precedence
explicit ``logging.python_format`` config ▸ platform-profile default ▸ library default (FR-RT-3).
Under the JSON sink the libraries do **not** install in-process size-rotation (FR-LOG-2) — the
cluster log agent owns rotation — so a read-only root FS never breaks logging (stdout only).
"""

import json
import logging
import logging.handlers
import time
from typing import Dict, Optional, Any
from pathlib import Path

from edgecommons.logs import LogPublishConfig

#: The case-insensitive token (FR-LOG-4) that selects the stdout-JSON sink. Mirrors
#: :data:`edgecommons.platform.resolver.LOGGING_FORMAT_JSON`; duplicated here as a plain literal so
#: this module carries no import-time dependency on the platform package.
JSON_FORMAT_TOKEN = "json"

#: Standard :class:`logging.LogRecord` attributes that are never echoed into the JSON object as
#: structured extras (they are either rendered explicitly or are noise). Used by
#: :class:`JsonLogFormatter` to surface caller-supplied ``extra=`` fields without leaking internals.
_RESERVED_RECORD_ATTRS = frozenset(
    logging.makeLogRecord({}).__dict__.keys()
) | {"message", "asctime", "taskName"}


class JsonLogFormatter(logging.Formatter):
    """Render each :class:`logging.LogRecord` as a single-line JSON object (FR-LOG-1).

    One JSON object is emitted per line (``json.dumps`` produces no embedded newlines for the scalar
    fields used here), with at least ``timestamp`` (ISO-8601 UTC), ``level``, ``logger`` and
    ``message``; plus ``thrown`` (the formatted exception) and ``stack`` when present. Best-effort
    **correlation fields** (``thing``/``pod``/``namespace``/``node``) are merged when non-empty and
    omitted otherwise — never emitted as empty/null noise (FR-LOG-3). Any caller-supplied ``extra=``
    record attributes are included too, so structured logging composes.

    Timestamps are always UTC (the converter is pinned to :func:`time.gmtime`) so the trailing ``Z``
    is correct regardless of the host's process-global ``logging.Formatter.converter``.

    Args:
        correlation: a mapping of correlation field name to value; falsy values are dropped.
    """

    converter = staticmethod(time.gmtime)

    def __init__(self, correlation: Optional[Dict[str, Any]] = None):
        super().__init__()
        # Snapshot only the non-empty correlation fields once, so the hot format() path stays cheap
        # and absent fields are never emitted (FR-LOG-3).
        self._correlation = {k: v for k, v in (correlation or {}).items() if v}

    def format(self, record: logging.LogRecord) -> str:
        ts = self.formatTime(record, "%Y-%m-%dT%H:%M:%S")
        obj: Dict[str, Any] = {
            "timestamp": f"{ts}.{int(record.msecs):03d}Z",
            "level": record.levelname,
            "logger": record.name,
            "message": record.getMessage(),
        }
        # Best-effort correlation fields (already pre-filtered to non-empty).
        obj.update(self._correlation)
        # Caller-supplied structured extras (logger.info(..., extra={...})).
        for key, value in record.__dict__.items():
            if key not in _RESERVED_RECORD_ATTRS and not key.startswith("_"):
                obj.setdefault(key, value)
        if record.exc_info:
            obj["thrown"] = self.formatException(record.exc_info)
        elif record.exc_text:
            obj["thrown"] = record.exc_text
        if record.stack_info:
            obj["stack"] = self.formatStack(record.stack_info)
        # default=str keeps the line valid JSON even for non-serializable extras (no logging failure).
        return json.dumps(obj, default=str)


class EnhancedLoggingConfiguration:
    """
    Enhanced logging configuration with advanced features.
    
    Supports file logging, per-logger levels, and dynamic reconfiguration.
    """
    
    DEFAULT_FORMAT = "%(asctime)s [%(levelname)s] %(name)s: %(message)s"
    DEFAULT_LEVEL = logging.INFO

    def __init__(
        self,
        logging_config: Optional[Dict[str, Any]] = None,
        platform_default_format: Optional[str] = None,
        correlation: Optional[Dict[str, Any]] = None,
    ):
        """
        Initialize enhanced logging configuration.

        Args:
            logging_config: Dictionary containing logging configuration
            platform_default_format: the platform-profile default ``logging.<lang>_format`` token
                (e.g. ``"json"`` on KUBERNETES) applied when the config omits ``python_format`` —
                the middle tier of the logging-format precedence (FR-RT-3 / FR-LOG-1). ``None``
                keeps the library default.
            correlation: best-effort correlation fields (``thing``/``pod``/``namespace``/``node``)
                merged into each line of the stdout-JSON sink; falsy values are dropped (FR-LOG-3).
        """
        self._config = logging_config or {}
        self._platform_default_format = platform_default_format
        self._correlation = correlation or {}
        self._parse_config()

    def _parse_config(self) -> None:
        """Parse the logging configuration dictionary."""
        # Basic logging settings
        self._level = self._parse_level(self._config.get('level', 'INFO'))
        # Per-language format key (replaces the former language-agnostic `format`). Precedence
        # (FR-RT-3): explicit `python_format` config > platform-profile default > library default.
        explicit_format = self._config.get('python_format')
        if explicit_format is not None:
            self._format = explicit_format
        elif self._platform_default_format is not None:
            self._format = self._platform_default_format
        else:
            self._format = self.DEFAULT_FORMAT
        # FR-LOG-4: the case-insensitive token `json` selects the stdout-JSON sink.
        self._json_sink = (
            isinstance(self._format, str) and self._format.strip().lower() == JSON_FORMAT_TOKEN
        )

        # File logging settings
        file_cfg = self._config.get('fileLogging', {})
        self._file_logging_enabled = file_cfg.get('enabled', False)
        self._log_file_path = file_cfg.get('filePath')
        self._max_file_size = file_cfg.get('maxFileSize', '10MB')
        self._backup_count = file_cfg.get('backupCount', 5)
        
        # Per-logger settings
        self._logger_levels = {}
        loggers_config = self._config.get('loggers', {})
        for logger_name, logger_config in loggers_config.items():
            if isinstance(logger_config, dict) and 'level' in logger_config:
                self._logger_levels[logger_name] = self._parse_level(logger_config['level'])
            elif isinstance(logger_config, str):
                self._logger_levels[logger_name] = self._parse_level(logger_config)
                
        # Global control settings
        self._global_control = self._config.get('globalControl', False)

        # Library-owned UNS log publisher configuration. This parser is strict even though
        # the broader logging section has historically been lenient in code; the packaged
        # schema carries the same strict surface for normal validation.
        self._publish_config = LogPublishConfig.from_logging_config(self._config)
        
    def _parse_level(self, level_str: str) -> int:
        """
        Parse logging level string to logging level constant.
        
        Args:
            level_str: String representation of logging level
            
        Returns:
            Logging level constant
        """
        if isinstance(level_str, int):
            return level_str
            
        level_map = {
            'TRACE': 5,
            'DEBUG': logging.DEBUG,
            'INFO': logging.INFO,
            'WARNING': logging.WARNING,
            'WARN': logging.WARNING,
            'ERROR': logging.ERROR,
            'CRITICAL': logging.CRITICAL,
            'FATAL': logging.CRITICAL
        }
        
        return level_map.get(level_str.upper(), self.DEFAULT_LEVEL)
        
    def _parse_file_size(self, size_str: str) -> int:
        """
        Parse file size string to bytes.
        
        Args:
            size_str: Size string like '10MB', '1GB', etc.
            
        Returns:
            Size in bytes
        """
        if isinstance(size_str, int):
            return size_str
            
        size_str = size_str.upper()
        multipliers = {
            'B': 1,
            'KB': 1024,
            'MB': 1024 * 1024,
            'GB': 1024 * 1024 * 1024
        }
        
        for suffix, multiplier in multipliers.items():
            if size_str.endswith(suffix):
                try:
                    return int(size_str[:-len(suffix)]) * multiplier
                except ValueError:
                    pass
                    
        # Default to 10MB if parsing fails
        return 10 * 1024 * 1024
        
    def configure_logging(self, config_manager=None) -> None:
        """
        Configure the logging system based on current settings.
        
        Args:
            config_manager: Optional config manager for template resolution
        """
        # Get root logger
        root_logger = logging.getLogger()
        
        # Clear existing handlers
        for handler in root_logger.handlers[:]:
            if getattr(handler, "_edgecommons_log_bus_handler", False):
                continue
            root_logger.removeHandler(handler)
            
        # Set root level low enough for the bus capture handler to see its configured
        # minLevel; console/file handlers still filter at the ordinary logging.level.
        bus_level = (
            self._publish_config.min_level_value
            if self._publish_config.enabled and self._publish_config.capture_native
            else self._level
        )
        root_logger.setLevel(min(self._level, bus_level))

        # Create the layout for the always-on stdout/console appender. FR-LOG-1: when the JSON sink
        # is selected the LAYOUT becomes one-JSON-object-per-line (with best-effort correlation
        # fields); otherwise the existing text/console layout is unchanged.
        if self._json_sink:
            formatter: logging.Formatter = JsonLogFormatter(self._correlation)
        else:
            formatter = logging.Formatter(self._format)

        # Add console handler
        console_handler = logging.StreamHandler()
        console_handler.setFormatter(formatter)
        console_handler.setLevel(self._level)
        root_logger.addHandler(console_handler)

        # FR-LOG-2: under the stdout-JSON sink (the KUBERNETES default) we never install in-process
        # size-rotation — the cluster log agent owns rotation/retention, and a rotating file handler
        # would also break on a read-only root FS. File logging stays available off the JSON sink.
        if self._json_sink and self._file_logging_enabled:
            logging.info(
                "stdout-JSON logging sink selected: skipping in-process file rotation "
                "(the cluster log agent owns rotation)."
            )

        # Add file handler if enabled (and not suppressed by the JSON sink)
        if self._file_logging_enabled and self._log_file_path and not self._json_sink:
            try:
                log_file_path = self._log_file_path
                
                # Resolve template variables if config manager available
                if config_manager and hasattr(config_manager, 'resolve_template'):
                    log_file_path = config_manager.resolve_template(log_file_path)
                    
                # Ensure directory exists
                Path(log_file_path).parent.mkdir(parents=True, exist_ok=True)
                
                # Create rotating file handler
                max_bytes = self._parse_file_size(self._max_file_size)
                file_handler = logging.handlers.RotatingFileHandler(
                    log_file_path,
                    maxBytes=max_bytes,
                    backupCount=self._backup_count
                )
                file_handler.setFormatter(formatter)
                file_handler.setLevel(self._level)
                root_logger.addHandler(file_handler)
                
                logging.info(f"File logging enabled: {log_file_path}")
                
            except Exception as e:
                logging.error(f"Failed to configure file logging: {e}")
                
        # Configure individual loggers
        for logger_name, level in self._logger_levels.items():
            logger = logging.getLogger(logger_name)
            logger.setLevel(level)
            
        if self._logger_levels:
            logging.info(f"Configured {len(self._logger_levels)} individual logger levels")
            
    def get_level(self) -> int:
        """Get the root logging level."""
        return self._level
        
    def get_format(self) -> str:
        """Get the effective logging format token (after precedence resolution)."""
        return self._format

    def is_json_sink(self) -> bool:
        """Whether the stdout-JSON sink is selected (the ``json`` format token; FR-LOG-1/4)."""
        return self._json_sink

    def is_file_logging_enabled(self) -> bool:
        """Check if file logging is enabled."""
        return self._file_logging_enabled
        
    def get_log_file_path(self) -> Optional[str]:
        """Get the log file path."""
        return self._log_file_path
        
    def get_logger_levels(self) -> Dict[str, int]:
        """Get per-logger level configuration."""
        return self._logger_levels.copy()
        
    def is_global_control_enabled(self) -> bool:
        """Check if global logging control is enabled."""
        return self._global_control

    def get_publish_config(self) -> LogPublishConfig:
        """Get the parsed ``logging.publish`` log-bus configuration."""
        return self._publish_config
        
    def to_dict(self) -> Dict[str, Any]:
        """
        Convert configuration to dictionary representation.
        
        Returns:
            Dictionary representation of the configuration
        """
        return {
            'level': logging.getLevelName(self._level),
            'python_format': self._format,
            'fileLogging': {
                'enabled': self._file_logging_enabled,
                'filePath': self._log_file_path,
                'maxFileSize': self._max_file_size,
                'backupCount': self._backup_count
            },
            'loggers': {
                name: logging.getLevelName(level) 
                for name, level in self._logger_levels.items()
            },
            'globalControl': self._global_control,
            'publish': {
                'enabled': self._publish_config.enabled,
                'destination': self._publish_config.destination,
                'minLevel': self._publish_config.min_level,
                'captureNative': self._publish_config.capture_native,
                'captureConsole': self._publish_config.capture_console,
                'maxRecordBytes': self._publish_config.max_record_bytes,
                'queue': {
                    'maxRecords': self._publish_config.queue.max_records,
                    'onFull': self._publish_config.queue.on_full,
                },
                'redaction': {
                    'enabled': self._publish_config.redaction.enabled,
                    'replacement': self._publish_config.redaction.replacement,
                    'extraPatterns': [
                        pattern.pattern
                        for pattern in self._publish_config.redaction.extra_patterns
                    ],
                },
            }
        }
