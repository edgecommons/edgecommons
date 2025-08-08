"""
Messaging service interface for ggcommons.

This interface defines the contract for messaging services,
providing abstraction for different messaging providers (IPC, MQTT).
"""

from abc import ABC, abstractmethod
from typing import Callable, Dict, Any, Optional
from concurrent.futures import Future
try:
    from awsiot.greengrasscoreipc.model import QOS
except ImportError:
    # Mock QOS for environments without AWS SDK
    class QOS:
        AT_MOST_ONCE = 0
        AT_LEAST_ONCE = 1


class IMessagingService(ABC):
    """
    Interface for messaging services.
    Provides abstraction for different messaging providers (IPC, MQTT).
    """

    @abstractmethod
    def subscribe(self, topic: str, handler: Callable[[str, Any], None], max_messages: int = 10) -> None:
        """
        Subscribes to IPC messages on the specified topic.
        
        Args:
            topic: Topic pattern (supports wildcards)
            handler: Message handler function
            max_messages: Maximum concurrent messages
            
        Raises:
            ValueError: If topic is None/empty or handler is None
        """
        pass

    @abstractmethod
    def subscribe_to_iot_core(self, topic: str, handler: Callable[[str, Any], None], 
                             qos: QOS, max_messages: int = 10) -> None:
        """
        Subscribes to IoT Core messages on the specified topic.
        
        Args:
            topic: Topic pattern
            handler: Message handler function
            qos: Quality of service level
            max_messages: Maximum concurrent messages
            
        Raises:
            ValueError: If topic is None/empty, handler is None, or qos is None
        """
        pass

    @abstractmethod
    def publish(self, topic: str, message: Any) -> None:
        """
        Publishes message via IPC.
        
        Args:
            topic: Target topic
            message: Message to publish
            
        Raises:
            ValueError: If topic is None/empty or message is None
        """
        pass

    @abstractmethod
    def publish_to_iot_core(self, topic: str, message: Any, qos: QOS) -> None:
        """
        Publishes message to IoT Core.
        
        Args:
            topic: Target topic
            message: Message to publish
            qos: Quality of service level
            
        Raises:
            ValueError: If topic is None/empty, message is None, or qos is None
        """
        pass

    @abstractmethod
    def publish_raw(self, topic: str, payload: Dict[str, Any]) -> None:
        """
        Publishes raw JSON payload via IPC.
        
        Args:
            topic: Target topic
            payload: JSON payload to publish
            
        Raises:
            ValueError: If topic is None/empty or payload is None
        """
        pass

    @abstractmethod
    def request(self, topic: str, message: Any) -> Future:
        """
        Sends request via IPC and returns future for response.
        
        Args:
            topic: Request topic
            message: Request message
            
        Returns:
            Future for the response
            
        Raises:
            ValueError: If topic is None/empty or message is None
        """
        pass

    @abstractmethod
    def request_from_iot_core(self, topic: str, message: Any) -> Future:
        """
        Sends request via IoT Core and returns future for response.
        
        Args:
            topic: Request topic
            message: Request message
            
        Returns:
            Future for the response
            
        Raises:
            ValueError: If topic is None/empty or message is None
        """
        pass

    @abstractmethod
    def reply(self, original_message: Any, reply_message: Any) -> None:
        """
        Sends reply to a received message.
        
        Args:
            original_message: The original message to reply to
            reply_message: The reply message
            
        Raises:
            ValueError: If original_message or reply_message is None
        """
        pass

    @abstractmethod
    def unsubscribe(self, topic_filter: str) -> None:
        """
        Unsubscribes from IPC messages on a topic.
        
        Args:
            topic_filter: The topic filter to unsubscribe from
            
        Raises:
            ValueError: If topic_filter is None/empty
        """
        pass

    @abstractmethod
    def unsubscribe_from_iot_core(self, topic_filter: str) -> None:
        """
        Unsubscribes from IoT Core messages on a topic.
        
        Args:
            topic_filter: The topic filter to unsubscribe from
            
        Raises:
            ValueError: If topic_filter is None/empty
        """
        pass

    @abstractmethod
    def topic_matches_filter(self, topic_filter: str, topic: str) -> bool:
        """
        Checks if a topic matches a topic filter pattern.
        
        Args:
            topic_filter: The topic filter pattern
            topic: The topic to check
            
        Returns:
            True if the topic matches the filter
            
        Raises:
            ValueError: If topic_filter or topic is None/empty
        """
        pass

    @abstractmethod
    def get_native_local_client(self) -> Optional[Any]:
        """
        Returns the native local messaging client.
        
        Returns:
            The native messaging client object or None if not available
        """
        pass

    @abstractmethod
    def get_native_iot_core_client(self) -> Optional[Any]:
        """
        Returns the native IoT Core messaging client.
        
        Returns:
            The native messaging client object or None if not available
        """
        pass