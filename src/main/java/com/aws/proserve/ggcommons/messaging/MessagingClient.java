/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.messaging;

import com.aws.proserve.ggcommons.ParsedCommandLine;
import com.aws.proserve.ggcommons.messaging.providers.mqtt.MqttProvider;
import com.aws.proserve.ggcommons.messaging.providers.greengrass.GreengrassIpcProvider;
import com.google.gson.JsonObject;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import software.amazon.awssdk.aws.greengrass.model.QOS;

import java.util.UUID;
import java.util.function.BiConsumer;

/**
 * A client for handling messaging operations in Greengrass components. This class provides methods for
 * publishing and subscribing to messages, making requests, and handling replies in both local and IoT Core
 * communication contexts.
 */
public class MessagingClient
{
    protected static final Logger LOGGER = LogManager.getLogger(MessagingClient.class);

    static MessagingProvider messagingProvider = null;

    /**
     * Initializes the messaging client with command line arguments and message reception settings.
     *
     * @param cmdLine Parsed command line arguments containing messaging configuration
     * @param receiveOwnMessages Flag indicating whether to receive messages published by this component
     */
    @SuppressWarnings("i18n")  // These are protocol identifiers that should not be localized
    public static void init(ParsedCommandLine cmdLine, boolean receiveOwnMessages)
    {
        String[] messagingArgs = cmdLine.messagingArgs;
        String clientId = cmdLine.thingName != null ? cmdLine.thingName : UUID.randomUUID().toString();
        switch (messagingArgs[0].toUpperCase()) {
            case "IPC":
                LOGGER.info("IPC specified in command line.  Using Greengrass IPC.");
                messagingProvider = new GreengrassIpcProvider(messagingArgs, receiveOwnMessages);
                break;
            case "MQTT":
                LOGGER.info("MQTT specified in command line.  Using MqttClient");
                messagingProvider = new MqttProvider(messagingArgs, clientId);
                break;
            default:
                LOGGER.fatal("Invalid com.aws.proseve.ggcommons.messaging provider specified in command line: must be either 'MQTT' or 'IPC'");
                System.exit(1);
        }
    }

    /**
     * Publishes a message to a specified topic.
     *
     * @param topic The topic to publish the message to
     * @param msg The message to publish
     */
    public static void publish(String topic, Message msg)
    {
        messagingProvider.publish(topic, msg);
        LOGGER.debug("Published IPC message on topic '{}': {}", topic, msg.toString());
    }

    /**
     * Publishes a message to AWS IoT Core with specified quality of service.
     *
     * @param topic The IoT Core topic to publish to
     * @param msg The message to publish
     * @param qos The quality of service level for message delivery
     */
    public static void publishToIotCore(String topic, Message msg, QOS qos)
    {
        messagingProvider.publishToIoTCore(topic, msg,  qos);
        LOGGER.debug("Published IoT Core message on topic '{}': {}", topic, msg.toString());
    }

    /**
     * Publishes a raw JSON object to a topic without wrapping it in a Message.
     *
     * @param topic The topic to publish to
     * @param metricObject The JSON object to publish
     */
    public static void publishRaw(String topic, JsonObject metricObject)
    {
        messagingProvider.publishRaw(topic, metricObject);
    }

    /**
     * Publishes a raw JSON object to a topic without wrapping it in a Message.
     *
     * @param topic The topic to publish to
     * @param metricObject The JSON object to publish
     */
    public static void publishToIotCoreRaw(String topic, JsonObject metricObject, QOS qos)
    {
        messagingProvider.publishToIoTCoreRaw(topic, metricObject, qos);
    }

    /**
     * Subscribes to messages on a topic with a callback for message handling.
     *
     * @param topicFilter The topic filter to subscribe to
     * @param callback The callback to invoke when messages are received
     */
    public static void subscribe(String topicFilter, BiConsumer<String, Message> callback)
    {
        subscribe(topicFilter, callback, -1);
    }
    public static void subscribe(String topicFilter, BiConsumer<String, Message> callback,
                                 int maxConcurrency)
    {
        messagingProvider.subscribe(topicFilter, callback, maxConcurrency);
        LOGGER.debug("Subscribed to IPC messages on topic filter {}", topicFilter);
    }

    /**
     * Subscribes to messages from IoT Core with specified quality of service.
     *
     * @param topicFilter The topic filter to subscribe to
     * @param callback The callback to invoke when messages are received
     * @param qos The quality of service level for the subscription
     */
    public static void subscribeToIoTCore(String topicFilter, BiConsumer<String, Message> callback, QOS qos)
    {
        subscribeToIoTCore(topicFilter, callback, qos, -1);
    }

    public static void subscribeToIoTCore(String topicFilter, BiConsumer<String, Message> callback, QOS qos,
                                          int maxConcurrency)
    {
        messagingProvider.subscribeToIoTCore(topicFilter, callback, qos, maxConcurrency);
        LOGGER.debug("Subscribed to IoT Core messages on topic filter {}", topicFilter);
    }

    /**
     * Sends a request message and returns a future for handling the reply.
     *
     * @param topic The topic to send the request to
     * @param request The request message
     * @return A ReplyFuture for handling the response
     */
    public static ReplyFuture request(String topic, Message request)
    {
        return messagingProvider.request(topic, request);
    }

    /**
     * Sends a request message to IoT Core and returns a future for handling the reply.
     *
     * @param topic The IoT Core topic to send the request to
     * @param request The request message
     * @return A ReplyFuture for handling the response
     */
    public static ReplyFuture requestFromIoTCore(String topic, Message request)
    {
        return messagingProvider.requestFromIoTCore(topic, request);
    }

    /**
     * Cancels a pending request and cleans up associated resources.
     *
     * @param replyFuture The ReplyFuture associated with the request to cancel
     */
    public static void cancelRequest(ReplyFuture replyFuture)
    {
        messagingProvider.cancelRequest(replyFuture);
    }

    public static void cancelRequestFromIoTCore(ReplyFuture replyFuture)
    {
        messagingProvider.cancelRequestFromIoTCore(replyFuture);
    }

    /**
     * Sends a reply to a received request message.
     *
     * @param request The original request message
     * @param reply The reply message
     */
    public static void reply(Message request, Message reply)
    {
        messagingProvider.reply(request, reply);
        LOGGER.debug("Published reply on topic '{}: {}", request.getHeader().getReplyTo(), reply.toString());
    }

    public static void replyToIoTCore(Message request, Message reply)
    {
        messagingProvider.replyToIoTCore(request, reply);
    }

    /**
     * Unsubscribes from messages on a topic.
     *
     * @param topicFilter The topic filter to unsubscribe from
     */
    public static void unsubscribe(String topicFilter)
    {
        messagingProvider.unsubscribe(topicFilter);
        LOGGER.debug("Unsubscribed to IPC messages on topic filter {}", topicFilter);
    }

    public static void unsubscribeFromIoTCore(String topicFilter)
    {
        messagingProvider.unsubscribeFromIoTCore(topicFilter);
        LOGGER.debug("Unsubscribed to IPC messages on topic filter {}", topicFilter);
    }

    /**
     * Checks if a topic matches a topic filter pattern.
     *
     * @param topicFilter The topic filter pattern
     * @param topic The topic to check
     * @return true if the topic matches the filter, false otherwise
     */
    public static boolean topicMatchesFilter(String topicFilter, String topic)
    {
        return MessagingProvider.topicMatchesFilter(topicFilter, topic);
    }

    /**
     * Returns the underlying native messaging client implementation.
     *
     * @return The native messaging client object
     */
    public static Object getNativeClient()
    {
        return messagingProvider.getNativeClient();
    }


}
