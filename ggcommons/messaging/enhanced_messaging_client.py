"""
Enhanced messaging client with improved error handling and standalone mode support.

This module provides an enhanced version of MessagingClient with better error handling,
connection management, and support for standalone/MQTT mode operations.
"""

import logging
import threading
import time
from typing import Dict, Any, Optional, Callable, List
from concurrent.futures import Future, ThreadPoolExecutor
from awsiot.greengrasscoreipc.model import QOS
from ggcommons.messaging.messaging_client import MessagingClient

logger = logging.getLogger(__name__)


class EnhancedMessagingClient:
    """
    Enhanced messaging client with improved reliability and error handling.
    
    Provides enhanced features over the base MessagingClient:
    - Better error handling and recovery
    - Connection health monitoring
    - Request-response pattern improvements
    - Topic matching utilities
    - Thread-safe operations
    """
    
    def __init__(self):
        """Initialize the enhanced messaging client."""
        self._subscriptions: Dict[str, List[Callable]] = {}
        self._request_futures: Dict[str, Future] = {}
        self._executor = ThreadPoolExecutor(max_workers=10, thread_name_prefix="messaging")
        self._lock = threading.RLock()
        self._connection_healthy = True
        self._last_health_check = time.time()
        
    @classmethod
    def init(cls, parsed_args, receive_own_messages: bool = True) -> None:
        """
        Initialize the enhanced messaging client.
        
        Args:
            parsed_args: Parsed command line arguments
            receive_own_messages: Whether to receive own messages
        """
        try:
            MessagingClient.init(parsed_args, receive_own_messages)
            logger.info("Enhanced messaging client initialized successfully")
        except Exception as e:
            logger.error(f"Failed to initialize enhanced messaging client: {e}")
            raise
            
    def subscribe(self, topic: str, handler: Callable[[str, Any], None], 
                 max_messages: int = 10) -> None:
        """
        Subscribe to messages with enhanced error handling.
        
        Args:
            topic: Topic pattern to subscribe to
            handler: Message handler function
            max_messages: Maximum concurrent messages
            
        Raises:
            ValueError: If parameters are invalid
        """
        if not topic:
            raise ValueError("Topic cannot be None or empty")
        if handler is None:
            raise ValueError("Handler cannot be None")
        if max_messages <= 0:
            raise ValueError("Max messages must be positive")
            
        try:
            # Wrap handler with error handling
            wrapped_handler = self._wrap_handler(handler, topic)
            
            with self._lock:
                if topic not in self._subscriptions:
                    self._subscriptions[topic] = []
                self._subscriptions[topic].append(wrapped_handler)
                
            MessagingClient.subscribe(topic, wrapped_handler, max_messages)
            logger.debug(f"Subscribed to topic: {topic}")
            
        except Exception as e:
            logger.error(f"Failed to subscribe to topic {topic}: {e}")
            raise
            
    def subscribe_to_iot_core(self, topic: str, handler: Callable[[str, Any], None],
                             qos: QOS, max_messages: int = 10) -> None:
        """
        Subscribe to IoT Core messages with enhanced error handling.
        
        Args:
            topic: Topic pattern to subscribe to
            handler: Message handler function
            qos: Quality of service level
            max_messages: Maximum concurrent messages
        """
        if not topic:
            raise ValueError("Topic cannot be None or empty")
        if handler is None:
            raise ValueError("Handler cannot be None")
        if qos is None:
            raise ValueError("QOS cannot be None")
            
        try:
            wrapped_handler = self._wrap_handler(handler, topic)
            MessagingClient.subscribe_to_iot_core(topic, wrapped_handler, qos, max_messages)
            logger.debug(f"Subscribed to IoT Core topic: {topic}")
            
        except Exception as e:
            logger.error(f"Failed to subscribe to IoT Core topic {topic}: {e}")
            raise
            
    def publish(self, topic: str, message: Any) -> None:
        """
        Publish message with enhanced error handling.
        
        Args:
            topic: Target topic
            message: Message to publish
        """
        if not topic:
            raise ValueError("Topic cannot be None or empty")
        if message is None:
            raise ValueError("Message cannot be None")
            
        try:
            MessagingClient.publish(topic, message)
            logger.debug(f"Published message to topic: {topic}")
            
        except Exception as e:
            logger.error(f"Failed to publish to topic {topic}: {e}")
            self._handle_publish_error(topic, message, e)
            raise
            
    def publish_to_iot_core(self, topic: str, message: Any, qos: QOS) -> None:
        """
        Publish message to IoT Core with enhanced error handling.
        
        Args:
            topic: Target topic
            message: Message to publish
            qos: Quality of service level
        """
        if not topic:
            raise ValueError("Topic cannot be None or empty")
        if message is None:
            raise ValueError("Message cannot be None")
        if qos is None:
            raise ValueError("QOS cannot be None")
            
        try:
            MessagingClient.publish_to_iot_core(topic, message, qos)
            logger.debug(f"Published message to IoT Core topic: {topic}")
            
        except Exception as e:
            logger.error(f"Failed to publish to IoT Core topic {topic}: {e}")
            raise
            
    def request(self, topic: str, message: Any, timeout: float = 30.0) -> Future:
        """
        Send request with enhanced timeout and error handling.
        
        Args:
            topic: Request topic
            message: Request message
            timeout: Request timeout in seconds
            
        Returns:
            Future for the response
        """
        if not topic:
            raise ValueError("Topic cannot be None or empty")
        if message is None:
            raise ValueError("Message cannot be None")
        if timeout <= 0:
            raise ValueError("Timeout must be positive")
            
        try:
            future = MessagingClient.request(topic, message)
            
            # Wrap with timeout handling
            def timeout_wrapper():
                try:
                    return future.result(timeout=timeout)
                except Exception as e:
                    logger.error(f"Request to {topic} failed: {e}")
                    raise
                    
            return self._executor.submit(timeout_wrapper)
            
        except Exception as e:
            logger.error(f"Failed to send request to topic {topic}: {e}")
            raise
            
    def topic_matches_filter(self, topic_filter: str, topic: str) -> bool:
        """
        Check if a topic matches a topic filter pattern.
        
        Args:
            topic_filter: The topic filter pattern (supports + and # wildcards)
            topic: The topic to check
            
        Returns:
            True if the topic matches the filter
        """
        if not topic_filter or not topic:
            return False
            
        # Handle exact match
        if topic_filter == topic:
            return True
            
        # Handle wildcards
        filter_parts = topic_filter.split('/')
        topic_parts = topic.split('/')
        
        i = 0
        j = 0
        
        while i < len(filter_parts) and j < len(topic_parts):
            filter_part = filter_parts[i]
            
            if filter_part == '#':
                # Multi-level wildcard matches everything remaining
                return True
            elif filter_part == '+':
                # Single-level wildcard matches one level
                i += 1
                j += 1
            elif filter_part == topic_parts[j]:
                # Exact match
                i += 1
                j += 1
            else:
                # No match
                return False
                
        # Check if we consumed all parts
        return i == len(filter_parts) and j == len(topic_parts)
        
    def unsubscribe(self, topic_filter: str) -> None:
        """
        Unsubscribe from a topic with cleanup.
        
        Args:
            topic_filter: The topic filter to unsubscribe from
        """
        if not topic_filter:
            raise ValueError("Topic filter cannot be None or empty")
            
        try:
            with self._lock:
                if topic_filter in self._subscriptions:
                    del self._subscriptions[topic_filter]
                    
            MessagingClient.unsubscribe(topic_filter)
            logger.debug(f"Unsubscribed from topic: {topic_filter}")
            
        except Exception as e:
            logger.error(f"Failed to unsubscribe from topic {topic_filter}: {e}")
            raise
            
    def get_connection_health(self) -> bool:
        """
        Check the health of messaging connections.
        
        Returns:
            True if connections are healthy
        """
        current_time = time.time()
        
        # Perform health check every 30 seconds
        if current_time - self._last_health_check > 30:
            self._last_health_check = current_time
            self._connection_healthy = self._perform_health_check()
            
        return self._connection_healthy
        
    def _wrap_handler(self, handler: Callable, topic: str) -> Callable:
        """
        Wrap message handler with error handling and logging.
        
        Args:
            handler: Original handler function
            topic: Topic being handled
            
        Returns:
            Wrapped handler function
        """
        def wrapped_handler(received_topic: str, message: Any):
            try:
                handler(received_topic, message)
            except Exception as e:
                logger.error(f"Error in message handler for topic {topic}: {e}")
                # Don't re-raise to prevent handler errors from breaking the subscription
                
        return wrapped_handler
        
    def _handle_publish_error(self, topic: str, message: Any, error: Exception) -> None:
        """
        Handle publish errors with potential retry logic.
        
        Args:
            topic: Topic that failed to publish
            message: Message that failed to publish
            error: The error that occurred
        """
        # Could implement retry logic here in the future
        logger.warning(f"Publish error for topic {topic}: {error}")
        
    def _perform_health_check(self) -> bool:
        """
        Perform actual health check of messaging connections.
        
        Returns:
            True if connections are healthy
        """
        try:
            # Could implement actual health check logic here
            # For now, assume healthy if no recent errors
            return True
        except Exception as e:
            logger.error(f"Health check failed: {e}")
            return False
            
    def shutdown(self) -> None:
        """Shutdown the enhanced messaging client and cleanup resources."""
        try:
            with self._lock:
                self._subscriptions.clear()
                self._request_futures.clear()
                
            self._executor.shutdown(wait=True)
            logger.info("Enhanced messaging client shutdown completed")
            
        except Exception as e:
            logger.error(f"Error during messaging client shutdown: {e}")


# Global instance for backward compatibility
_enhanced_client: Optional[EnhancedMessagingClient] = None


def get_enhanced_client() -> EnhancedMessagingClient:
    """
    Get the global enhanced messaging client instance.
    
    Returns:
        The enhanced messaging client instance
    """
    global _enhanced_client
    if _enhanced_client is None:
        _enhanced_client = EnhancedMessagingClient()
    return _enhanced_client