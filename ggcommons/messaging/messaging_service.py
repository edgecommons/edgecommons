"""
Messaging service implementation.

This module provides a concrete implementation of IMessagingService
that wraps the existing MessagingClient functionality.
"""

from typing import Callable, Dict, Any, Optional
from concurrent.futures import Future
from awsiot.greengrasscoreipc.model import QOS
from ggcommons.interfaces.i_messaging_service import IMessagingService
from ggcommons.messaging.messaging_client import MessagingClient


class MessagingService(IMessagingService):
    """
    Service implementation that wraps MessagingClient to provide the IMessagingService interface.
    This allows for dependency injection while maintaining backward compatibility.
    """

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
        if not topic:
            raise ValueError("Topic cannot be None or empty")
        if handler is None:
            raise ValueError("Handler cannot be None")
        if max_messages <= 0:
            raise ValueError("Max messages must be positive")
            
        MessagingClient.subscribe(topic, handler, max_messages)

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
        if not topic:
            raise ValueError("Topic cannot be None or empty")
        if handler is None:
            raise ValueError("Handler cannot be None")
        if qos is None:
            raise ValueError("QOS cannot be None")
        if max_messages <= 0:
            raise ValueError("Max messages must be positive")
            
        MessagingClient.subscribe_to_iot_core(topic, handler, qos, max_messages)

    def publish(self, topic: str, message: Any) -> None:
        """
        Publishes message via IPC.
        
        Args:
            topic: Target topic
            message: Message to publish
            
        Raises:
            ValueError: If topic is None/empty or message is None
        """
        if not topic:
            raise ValueError("Topic cannot be None or empty")
        if message is None:
            raise ValueError("Message cannot be None")
            
        MessagingClient.publish(topic, message)

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
        if not topic:
            raise ValueError("Topic cannot be None or empty")
        if message is None:
            raise ValueError("Message cannot be None")
        if qos is None:
            raise ValueError("QOS cannot be None")
            
        MessagingClient.publish_to_iot_core(topic, message, qos)

    def publish_raw(self, topic: str, payload: Dict[str, Any]) -> None:
        """
        Publishes raw JSON payload via IPC.
        
        Args:
            topic: Target topic
            payload: JSON payload to publish
            
        Raises:
            ValueError: If topic is None/empty or payload is None
        """
        if not topic:
            raise ValueError("Topic cannot be None or empty")
        if payload is None:
            raise ValueError("Payload cannot be None")
            
        MessagingClient.publish_raw(topic, payload)

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
        if not topic:
            raise ValueError("Topic cannot be None or empty")
        if message is None:
            raise ValueError("Message cannot be None")
            
        return MessagingClient.request(topic, message)

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
        if not topic:
            raise ValueError("Topic cannot be None or empty")
        if message is None:
            raise ValueError("Message cannot be None")
            
        return MessagingClient.request_from_iot_core(topic, message)

    def reply(self, original_message: Any, reply_message: Any) -> None:
        """
        Sends reply to a received message.
        
        Args:
            original_message: The original message to reply to
            reply_message: The reply message
            
        Raises:
            ValueError: If original_message or reply_message is None
        """
        if original_message is None:
            raise ValueError("Original message cannot be None")
        if reply_message is None:
            raise ValueError("Reply message cannot be None")
            
        MessagingClient.reply(original_message, reply_message)

    def unsubscribe(self, topic_filter: str) -> None:
        """
        Unsubscribes from IPC messages on a topic.
        
        Args:
            topic_filter: The topic filter to unsubscribe from
            
        Raises:
            ValueError: If topic_filter is None/empty
        """
        if not topic_filter:
            raise ValueError("Topic filter cannot be None or empty")
            
        MessagingClient.unsubscribe(topic_filter)

    def unsubscribe_from_iot_core(self, topic_filter: str) -> None:
        """
        Unsubscribes from IoT Core messages on a topic.
        
        Args:
            topic_filter: The topic filter to unsubscribe from
            
        Raises:
            ValueError: If topic_filter is None/empty
        """
        if not topic_filter:
            raise ValueError("Topic filter cannot be None or empty")
            
        MessagingClient.unsubscribe_from_iot_core(topic_filter)

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
        if not topic_filter:
            raise ValueError("Topic filter cannot be None or empty")
        if not topic:
            raise ValueError("Topic cannot be None or empty")
            
        return MessagingClient.topic_matches_filter(topic_filter, topic)

    def get_native_local_client(self) -> Optional[Any]:
        """
        Returns the native local messaging client.
        
        Returns:
            The native messaging client object or None if not available
        """
        return MessagingClient.get_native_local_client()

    def get_native_iot_core_client(self) -> Optional[Any]:
        """
        Returns the native IoT Core messaging client.
        
        Returns:
            The native messaging client object or None if not available
        """
        return MessagingClient.get_native_iot_core_client()