"""
File watcher utility for monitoring configuration file changes.

This module provides file watching capabilities for dynamic configuration
reloading and other file-based monitoring needs.
"""

import logging
import os
import threading
import time
from typing import Callable, Optional, Dict, Any
from pathlib import Path

logger = logging.getLogger(__name__)


class FileChangeHandler:
    """
    Interface for handling file change events.
    """
    
    def on_file_changed(self, file_path: str) -> None:
        """
        Called when a monitored file changes.
        
        Args:
            file_path: Path to the changed file
        """
        pass
        
    def on_file_created(self, file_path: str) -> None:
        """
        Called when a monitored file is created.
        
        Args:
            file_path: Path to the created file
        """
        pass
        
    def on_file_deleted(self, file_path: str) -> None:
        """
        Called when a monitored file is deleted.
        
        Args:
            file_path: Path to the deleted file
        """
        pass


class FileWatcher:
    """
    Simple file watcher implementation using polling.
    
    Monitors files for changes and notifies registered handlers.
    Uses polling approach for cross-platform compatibility.
    """
    
    def __init__(self, poll_interval: float = 1.0):
        """
        Initialize file watcher.
        
        Args:
            poll_interval: Polling interval in seconds
        """
        self._poll_interval = poll_interval
        self._watched_files: Dict[str, Dict[str, Any]] = {}
        self._handlers: Dict[str, FileChangeHandler] = {}
        self._running = False
        self._thread: Optional[threading.Thread] = None
        self._lock = threading.RLock()
        
    def watch_file(self, file_path: str, handler: FileChangeHandler) -> None:
        """
        Start watching a file for changes.
        
        Args:
            file_path: Path to file to watch
            handler: Handler for file change events
            
        Raises:
            ValueError: If file_path or handler is None
            FileNotFoundError: If file doesn't exist
        """
        if not file_path:
            raise ValueError("File path cannot be None or empty")
        if handler is None:
            raise ValueError("Handler cannot be None")
            
        file_path = os.path.abspath(file_path)
        
        if not os.path.exists(file_path):
            raise FileNotFoundError(f"File not found: {file_path}")
            
        with self._lock:
            # Get initial file stats
            stat = os.stat(file_path)
            self._watched_files[file_path] = {
                'mtime': stat.st_mtime,
                'size': stat.st_size,
                'exists': True
            }
            self._handlers[file_path] = handler
            
        logger.debug(f"Started watching file: {file_path}")
        
    def unwatch_file(self, file_path: str) -> None:
        """
        Stop watching a file.
        
        Args:
            file_path: Path to file to stop watching
        """
        if not file_path:
            return
            
        file_path = os.path.abspath(file_path)
        
        with self._lock:
            self._watched_files.pop(file_path, None)
            self._handlers.pop(file_path, None)
            
        logger.debug(f"Stopped watching file: {file_path}")
        
    def start(self) -> None:
        """Start the file watcher thread."""
        with self._lock:
            if self._running:
                return
                
            self._running = True
            self._thread = threading.Thread(target=self._watch_loop, daemon=True)
            self._thread.start()
            
        logger.info("File watcher started")
        
    def stop(self) -> None:
        """Stop the file watcher thread."""
        with self._lock:
            self._running = False
            
        if self._thread and self._thread.is_alive():
            self._thread.join(timeout=5.0)
            
        logger.info("File watcher stopped")
        
    def is_running(self) -> bool:
        """
        Check if file watcher is running.
        
        Returns:
            True if watcher is running
        """
        return self._running
        
    def _watch_loop(self) -> None:
        """Main watching loop that runs in separate thread."""
        logger.debug("File watcher loop started")
        
        try:
            while self._running:
                self._check_files()
                time.sleep(self._poll_interval)
                
        except Exception as e:
            logger.error(f"Error in file watcher loop: {e}")
        finally:
            logger.debug("File watcher loop stopped")
            
    def _check_files(self) -> None:
        """Check all watched files for changes."""
        with self._lock:
            files_to_check = list(self._watched_files.items())
            
        for file_path, file_info in files_to_check:
            try:
                self._check_single_file(file_path, file_info)
            except Exception as e:
                logger.error(f"Error checking file {file_path}: {e}")
                
    def _check_single_file(self, file_path: str, file_info: Dict[str, Any]) -> None:
        """
        Check a single file for changes.
        
        Args:
            file_path: Path to file to check
            file_info: Current file information
        """
        handler = self._handlers.get(file_path)
        if handler is None:
            return
            
        file_exists = os.path.exists(file_path)
        
        if not file_exists and file_info['exists']:
            # File was deleted
            with self._lock:
                file_info['exists'] = False
            try:
                handler.on_file_deleted(file_path)
            except Exception as e:
                logger.error(f"Error in file deleted handler for {file_path}: {e}")
                
        elif file_exists and not file_info['exists']:
            # File was created
            stat = os.stat(file_path)
            with self._lock:
                file_info['mtime'] = stat.st_mtime
                file_info['size'] = stat.st_size
                file_info['exists'] = True
            try:
                handler.on_file_created(file_path)
            except Exception as e:
                logger.error(f"Error in file created handler for {file_path}: {e}")
                
        elif file_exists:
            # Check for modifications
            stat = os.stat(file_path)
            
            if (stat.st_mtime != file_info['mtime'] or 
                stat.st_size != file_info['size']):
                # File was modified
                with self._lock:
                    file_info['mtime'] = stat.st_mtime
                    file_info['size'] = stat.st_size
                try:
                    handler.on_file_changed(file_path)
                except Exception as e:
                    logger.error(f"Error in file changed handler for {file_path}: {e}")


class ConfigFileWatcher(FileChangeHandler):
    """
    Specialized file watcher for configuration files.
    
    Provides configuration-specific handling with debouncing and validation.
    """
    
    def __init__(self, config_file_path: str, 
                 change_callback: Callable[[str], None],
                 debounce_seconds: float = 2.0):
        """
        Initialize configuration file watcher.
        
        Args:
            config_file_path: Path to configuration file
            change_callback: Callback function for configuration changes
            debounce_seconds: Debounce period to avoid rapid-fire changes
        """
        self.config_file_path = config_file_path
        self.change_callback = change_callback
        self.debounce_seconds = debounce_seconds
        self._last_change_time = 0
        self._debounce_timer: Optional[threading.Timer] = None
        self._lock = threading.Lock()
        
    def on_file_changed(self, file_path: str) -> None:
        """Handle configuration file changes with debouncing."""
        current_time = time.time()
        
        with self._lock:
            self._last_change_time = current_time
            
            # Cancel existing timer
            if self._debounce_timer:
                self._debounce_timer.cancel()
                
            # Start new debounce timer
            self._debounce_timer = threading.Timer(
                self.debounce_seconds,
                self._handle_debounced_change
            )
            self._debounce_timer.start()
            
        logger.debug(f"Configuration file change detected: {file_path}")
        
    def _handle_debounced_change(self) -> None:
        """Handle debounced configuration change."""
        try:
            logger.info(f"Processing configuration file change: {self.config_file_path}")
            self.change_callback(self.config_file_path)
        except Exception as e:
            logger.error(f"Error processing configuration change: {e}")
            
    def on_file_created(self, file_path: str) -> None:
        """Handle configuration file creation."""
        logger.info(f"Configuration file created: {file_path}")
        self.on_file_changed(file_path)
        
    def on_file_deleted(self, file_path: str) -> None:
        """Handle configuration file deletion."""
        logger.warning(f"Configuration file deleted: {file_path}")
        # Could implement fallback to default configuration here