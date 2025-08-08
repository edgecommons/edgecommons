import logging
import uuid
from argparse import Namespace
from typing import Callable, List, Optional

from ggcommons.messaging.message import Message
from ggcommons.messaging.messaging_provider import MessagingProvider
from ggcommons.messaging.messaging_config import MessagingConfiguration
from ggcommons.messaging.providers.greengrass.greengrass_ipc import (
    GreengrassIpcProvider,
)
from ggcommons.messaging.providers.mqtt import MqttProvider
from ggcommons.messaging.providers.standalone_provider import StandaloneProvider
from ggcommons.utils.iou import Iou
from awsiot.greengrasscoreipc.model import QOS

logger = logging.getLogger("MessagingClient")


class MessagingClient:
    _messaging_provider: MessagingProvider = None

    @staticmethod
    def init(args: Namespace, standalone_config_path: str = None, receive_own_messages=False) -> MessagingProvider:
        """Initialize messaging client based on mode and configuration."""
        mode = getattr(args, 'mode', None)
        thing_name = getattr(args, 'thing', None)
        
        logger.info(f"Initializing MessagingClient - mode: {mode}, thing_name: {thing_name}, receive_own_messages: {receive_own_messages}")
        
        # Determine messaging mode
        if mode and len(mode) > 0 and mode[0].upper() == 'STANDALONE':
            logger.info(f"Configuring STANDALONE mode with dual broker support - config file: {standalone_config_path}")
            if not standalone_config_path:
                logger.error("STANDALONE mode specified but no config file path provided")
                raise RuntimeError("STANDALONE mode requires standalone config file path")
            
            logger.debug(f"Loading messaging configuration from: {standalone_config_path}")
            messaging_config = MessagingClient._get_messaging_config(standalone_config_path)
            
            logger.info("Creating StandaloneProvider for dual broker messaging")
            MessagingClient._messaging_provider = StandaloneProvider(messaging_config, thing_name)
            logger.info("STANDALONE mode messaging provider initialized successfully")
        else:
            # Default to IPC mode
            logger.info(f"Configuring Greengrass IPC mode - receive_own_messages: {receive_own_messages}")
            MessagingClient._messaging_provider = GreengrassIpcProvider(
                receive_own_messages
            )
            logger.info("Greengrass IPC messaging provider initialized successfully")
        
        if MessagingClient._messaging_provider is None:
            logger.error("Failed to create messaging provider - provider is None")
            raise RuntimeError("Failed to initialize messaging provider")
        
        logger.info(f"MessagingClient initialization completed - provider type: {type(MessagingClient._messaging_provider).__name__}")
        return MessagingClient._messaging_provider
    
    @staticmethod
    def _get_messaging_config(standalone_config_path: str) -> MessagingConfiguration:
        """Get messaging configuration from standalone config file."""
        logger.debug(f"Loading messaging configuration from file: {standalone_config_path}")
        
        try:
            config = MessagingConfiguration.load_from_file(standalone_config_path)
            logger.debug(f"Successfully loaded messaging configuration from {standalone_config_path}")
            
            logger.debug("Validating messaging configuration")
            if not config.validate():
                logger.error(f"Messaging configuration validation failed for file: {standalone_config_path}")
                raise RuntimeError("Invalid messaging configuration")
            
            logger.info(f"Messaging configuration loaded and validated successfully from: {standalone_config_path}")
            return config
            
        except Exception as e:
            logger.error(f"Failed to load messaging configuration from {standalone_config_path}: {e}")
            raise RuntimeError(f"STANDALONE mode requires valid messaging configuration: {e}")

    @staticmethod
    def shutdown():
        MessagingClient._messaging_provider.disconnect()
        MessagingClient._messaging_provider = None

    @staticmethod
    def get_messaging_provider() -> MessagingProvider:
        return MessagingClient._messaging_provider

    @staticmethod
    def publish(topic: str, msg: Message):
        logger.debug(f"Publishing message to topic: {topic}")
        MessagingClient._messaging_provider.publish(topic, msg)

    @staticmethod
    def publish_raw(topic: str, msg: dict):
        MessagingClient._messaging_provider.publish_raw(topic, msg)

    @staticmethod
    def publish_to_iot_core(topic: str, msg: Message, qos: str):
        logger.debug(f"Publishing message to IoT Core topic: {topic}, QoS: {qos}")
        MessagingClient._messaging_provider.publish_to_iot_core(topic, msg, qos)

    @staticmethod
    def publish_to_iot_core_raw(topic: str, msg: dict, qos: str):
        MessagingClient._messaging_provider.publish_to_iot_core_raw(topic, msg, qos)

    @staticmethod
    def subscribe(
        topic: str,
        callback: Callable[[str, Message], None],
        max_concurrency: int = None,
    ):
        logger.debug(f"Subscribing to topic: {topic}, max_concurrency: {max_concurrency}")
        MessagingClient._messaging_provider.subscribe(topic, callback, max_concurrency)

    @staticmethod
    def subscribe_to_iot_core(
        topic: str,
        callback: Callable[[str, Message], None],
        qos: QOS,
        max_concurrency: int = None,
    ):
        logger.debug(f"Subscribing to IoT Core topic: {topic}, QoS: {qos}, max_concurrency: {max_concurrency}")
        MessagingClient._messaging_provider.subscribe_to_iot_core(
            topic, callback, qos, max_concurrency
        )

    @staticmethod
    def unsubscribe(topic: str):
        MessagingClient._messaging_provider.unsubscribe(topic)

    @staticmethod
    def unsubscribe_from_iot_core(topic: str):
        MessagingClient._messaging_provider.unsubscribe_from_iot_core(topic)

    @staticmethod
    def request(topic: str, msg: Message) -> Iou:
        logger.debug(f"Sending request to topic: {topic}")
        return MessagingClient._messaging_provider.request(topic, msg)

    @staticmethod
    def request_from_iot_core(topic: str, msg: Message) -> Iou:
        logger.debug(f"Sending request to IoT Core topic: {topic}")
        return MessagingClient._messaging_provider.request_from_iot_core(topic, msg)

    @staticmethod
    def cancel_request(iou: Iou) -> Iou:
        return MessagingClient._messaging_provider.cancel_request(iou)

    @staticmethod
    def cancel_request_from_iot_core(iou: Iou) -> Iou:
        return MessagingClient._messaging_provider.cancel_request(iou)

    @staticmethod
    def reply(request: Message, reply: Message):
        MessagingClient._messaging_provider.reply(request, reply)

    @staticmethod
    def reply_to_iot_core(request: Message, reply: Message):
        MessagingClient._messaging_provider.reply_to_iot_core(request, reply)

    @staticmethod
    def topic_matches_sub(sub: str, topic: str) -> bool:
        return MessagingProvider.topic_matches_sub(sub, topic)

    @staticmethod
    def get_native_client():
        return MessagingClient._messaging_provider.get_native_client()
