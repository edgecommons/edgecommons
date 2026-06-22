"""
Enhanced logging configuration with advanced features.

This module provides enhanced logging configuration with support for:
- File logging with rotation
- Per-logger level configuration
- Global logging control
- Dynamic reconfiguration
"""

import logging
import logging.handlers
from typing import Dict, Optional, Any
from pathlib import Path


class EnhancedLoggingConfiguration:
    """
    Enhanced logging configuration with advanced features.
    
    Supports file logging, per-logger levels, and dynamic reconfiguration.
    """
    
    DEFAULT_FORMAT = "%(asctime)s [%(levelname)s] %(name)s: %(message)s"
    DEFAULT_LEVEL = logging.INFO
    
    def __init__(self, logging_config: Optional[Dict[str, Any]] = None):
        """
        Initialize enhanced logging configuration.
        
        Args:
            logging_config: Dictionary containing logging configuration
        """
        self._config = logging_config or {}
        self._parse_config()
        
    def _parse_config(self) -> None:
        """Parse the logging configuration dictionary."""
        # Basic logging settings
        self._level = self._parse_level(self._config.get('level', 'INFO'))
        # Per-language format key (replaces the former language-agnostic `format`).
        self._format = self._config.get('python_format', self.DEFAULT_FORMAT)
        
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
            root_logger.removeHandler(handler)
            
        # Set root level
        root_logger.setLevel(self._level)
        
        # Create formatter
        formatter = logging.Formatter(self._format)
        
        # Add console handler
        console_handler = logging.StreamHandler()
        console_handler.setFormatter(formatter)
        console_handler.setLevel(self._level)
        root_logger.addHandler(console_handler)
        
        # Add file handler if enabled
        if self._file_logging_enabled and self._log_file_path:
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
        """Get the logging format string."""
        return self._format
        
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
            'globalControl': self._global_control
        }