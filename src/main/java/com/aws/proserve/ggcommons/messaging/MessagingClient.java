/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.messaging;

import com.aws.proserve.ggcommons.ParsedCommandLine;
import com.aws.proserve.ggcommons.messaging.providers.standalone.StandaloneMessagingProvider;
import com.aws.proserve.ggcommons.messaging.providers.greengrass.GreengrassMessagingProvider;
import com.google.gson.JsonObject;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import software.amazon.awssdk.aws.greengrass.model.QOS;

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
    public static void init(ParsedCommandLine cmdLine, boolean receiveOwnMessages)
    {
        switch (cmdLine.mode) {
            case GREENGRASS:
                LOGGER.info("GREENGRASS mode specified. Using Greengrass IPC.");
                messagingProvider = new GreengrassMessagingProvider(receiveOwnMessages);
                break;
            case STANDALONE:
                LOGGER.info("STANDALONE mode specified. Using dual MQTT clients.");
                try {
                    MessagingConfiguration config = MessagingConfiguration.loadFromFile(cmdLine.standaloneConfigPath);
                    messagingProvider = new StandaloneMessagingProvider(config, cmdLine.thingName);
                } catch (Exception e) {
                    LOGGER.fatal("Failed to load standalone messaging configuration: {}", e.getMessage());
                    System.exit(1);
                }
                break;
            default:
                LOGGER.fatal("Invalid mode specified: {}", cmdLine.mode);
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
        MessagingProvider provider = messagingProvider;
        if (provider == null) {
            throw new IllegalStateException("MessagingClient not initialized");
        }
        provider.publish(topic, msg);
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
        MessagingProvider provider = messagingProvider;
        if (provider == null) {
            throw new IllegalStateException("MessagingClient not initialized");
        }
        provider.publishToIoTCore(topic, msg,  qos);
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
        MessagingProvider provider = messagingProvider;
        if (provider == null) {
            throw new IllegalStateException("MessagingClient not initialized");
        }
        provider.publishRaw(topic, metricObject);
    }

    /**
     * Publishes a raw JSON object to a topic without wrapping it in a Message.
     *
     * @param topic The topic to publish to
     * @param metricObject The JSON object to publish
     */
    public static void publishToIotCoreRaw(String topic, JsonObject metricObject, QOS qos)
    {
        MessagingProvider provider = messagingProvider;
        if (provider == null) {
            throw new IllegalStateException("MessagingClient not initialized");
        }
        provider.publishToIoTCoreRaw(topic, metricObject, qos);
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
        MessagingProvider provider = messagingProvider;
        if (provider == null) {
            throw new IllegalStateException("MessagingClient not initialized");
        }
        provider.subscribe(topicFilter, callback, maxConcurrency);
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
        MessagingProvider provider = messagingProvider;
        if (provider == null) {
            throw new IllegalStateException("MessagingClient not initialized");
        }
        provider.subscribeToIoTCore(topicFilter, callback, qos, maxConcurrency);
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
        MessagingProvider provider = messagingProvider;
        if (provider == null) {
            throw new IllegalStateException("MessagingClient not initialized");
        }
        return provider.request(topic, request);
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
        MessagingProvider provider = messagingProvider;
        if (provider == null) {
            throw new IllegalStateException("MessagingClient not initialized");
        }
        return provider.requestFromIoTCore(topic, request);
    }

    /**
     * Cancels a pending request and cleans up associated resources.
     *
     * @param replyFuture The ReplyFuture associated with the request to cancel
     */
    public static void cancelRequest(ReplyFuture replyFuture)
    {
        MessagingProvider provider = messagingProvider;
        if (provider == null) {
            throw new IllegalStateException("MessagingClient not initialized");
        }
        provider.cancelRequest(replyFuture);
    }

    public static void cancelRequestFromIoTCore(ReplyFuture replyFuture)
    {
        MessagingProvider provider = messagingProvider;
        if (provider == null) {
            throw new IllegalStateException("MessagingClient not initialized");
        }
        provider.cancelRequestFromIoTCore(replyFuture);
    }

    /**
     * Sends a reply to a received request message.
     *
     * @param request The original request message
     * @param reply The reply message
     */
    public static void reply(Message request, Message reply)
    {
        MessagingProvider provider = messagingProvider;
        if (provider == null) {
            throw new IllegalStateException("MessagingClient not initialized");
        }
        provider.reply(request, reply);
        LOGGER.debug("Published reply on topic '{}: {}", request.getHeader().getReplyTo(), reply.toString());
    }

    public static void replyToIoTCore(Message request, Message reply)
    {
        MessagingProvider provider = messagingProvider;
        if (provider == null) {
            throw new IllegalStateException("MessagingClient not initialized");
        }
        provider.replyToIoTCore(request, reply);
    }

    /**
     * Unsubscribes from messages on a topic.
     *
     * @param topicFilter The topic filter to unsubscribe from
     */
    public static void unsubscribe(String topicFilter)
    {
        MessagingProvider provider = messagingProvider;
        if (provider == null) {
            throw new IllegalStateException("MessagingClient not initialized");
        }
        provider.unsubscribe(topicFilter);
        LOGGER.debug("Unsubscribed to IPC messages on topic filter {}", topicFilter);
    }

    public static void unsubscribeFromIoTCore(String topicFilter)
    {
        MessagingProvider provider = messagingProvider;
        if (provider == null) {
            throw new IllegalStateException("MessagingClient not initialized");
        }
        provider.unsubscribeFromIoTCore(topicFilter);
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
     * Returns the underlying native local messaging client implementation.
     *
     * @return The native messaging client object
     */
    public static Object getNativeLocalClient()
    {
        MessagingProvider provider = messagingProvider;
        if (provider == null) {
            throw new IllegalStateException("MessagingClient not initialized");
        }
        return provider.getNativeLocalClient();
    }

    /**
     * Returns the underlying native iot core messaging client implementation.
     *
     * @return The native messaging client object
     */
    public static Object getNativeIotCoreClient()
    {
        MessagingProvider provider = messagingProvider;
        if (provider == null) {
            throw new IllegalStateException("MessagingClient not initialized");
        }
        return provider.getNativeIotCoreClient();
    }

}
