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

    private MessagingProvider messagingProvider;

    /**
     * Protected no-arg constructor for testing/subclassing (e.g. mock messaging clients).
     * Leaves the underlying provider null; subclasses are expected to override the messaging methods.
     */
    protected MessagingClient() {
    }

    /**
     * Package-private constructor for builder pattern.
     */
    MessagingClient(ParsedCommandLine cmdLine, boolean receiveOwnMessages) {
        switch (cmdLine.mode) {
            case GREENGRASS:
                LOGGER.info("GREENGRASS mode specified. Using Greengrass IPC.");
                this.messagingProvider = new GreengrassMessagingProvider(receiveOwnMessages);
                break;
            case STANDALONE:
                LOGGER.info("STANDALONE mode specified. Using dual MQTT clients.");
                try {
                    MessagingConfiguration config = MessagingConfiguration.loadFromFile(cmdLine.standaloneConfigPath);
                    this.messagingProvider = new StandaloneMessagingProvider(config, cmdLine.thingName);
                } catch (Exception e) {
                    LOGGER.fatal("Failed to load standalone messaging configuration: {}", e.getMessage());
                    throw new RuntimeException("Failed to load standalone messaging configuration: " + e.getMessage(), e);
                }
                break;
            default:
                LOGGER.fatal("Invalid mode specified: {}", cmdLine.mode);
                throw new RuntimeException("Invalid mode specified: " + cmdLine.mode);
        }
    }

    /**
     * Publishes a message to a specified topic.
     *
     * @param topic The topic to publish the message to
     * @param msg The message to publish
     */
    public void publish(String topic, Message msg)
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
    public void publishToIotCore(String topic, Message msg, QOS qos)
    {
        messagingProvider.publishToIoTCore(topic, msg, qos);
        LOGGER.debug("Published IoT Core message on topic '{}': {}", topic, msg.toString());
    }

    /**
     * Publishes a raw JSON object to a topic without wrapping it in a Message.
     *
     * @param topic The topic to publish to
     * @param metricObject The JSON object to publish
     */
    public void publishRaw(String topic, JsonObject metricObject)
    {
        messagingProvider.publishRaw(topic, metricObject);
    }

    /**
     * Publishes a raw JSON object to a topic without wrapping it in a Message.
     *
     * @param topic The topic to publish to
     * @param metricObject The JSON object to publish
     */
    public void publishToIotCoreRaw(String topic, JsonObject metricObject, QOS qos)
    {
        messagingProvider.publishToIoTCoreRaw(topic, metricObject, qos);
    }

    /**
     * Subscribes to messages on a topic with a callback for message handling.
     *
     * @param topicFilter The topic filter to subscribe to
     * @param callback The callback to invoke when messages are received
     */
    public void subscribe(String topicFilter, BiConsumer<String, Message> callback)
    {
        subscribe(topicFilter, callback, -1);
    }
    
    public void subscribe(String topicFilter, BiConsumer<String, Message> callback, int maxConcurrency)
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
    public void subscribeToIoTCore(String topicFilter, BiConsumer<String, Message> callback, QOS qos)
    {
        subscribeToIoTCore(topicFilter, callback, qos, -1);
    }

    public void subscribeToIoTCore(String topicFilter, BiConsumer<String, Message> callback, QOS qos, int maxConcurrency)
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
    public ReplyFuture request(String topic, Message request)
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
    public ReplyFuture requestFromIoTCore(String topic, Message request)
    {
        return messagingProvider.requestFromIoTCore(topic, request);
    }

    /**
     * Cancels a pending request and cleans up associated resources.
     *
     * @param replyFuture The ReplyFuture associated with the request to cancel
     */
    public void cancelRequest(ReplyFuture replyFuture)
    {
        messagingProvider.cancelRequest(replyFuture);
    }

    public void cancelRequestFromIoTCore(ReplyFuture replyFuture)
    {
        messagingProvider.cancelRequestFromIoTCore(replyFuture);
    }

    /**
     * Sends a reply to a received request message.
     *
     * @param request The original request message
     * @param reply The reply message
     */
    public void reply(Message request, Message reply)
    {
        messagingProvider.reply(request, reply);
        LOGGER.debug("Published reply on topic '{}: {}", request.getHeader().getReplyTo(), reply.toString());
    }

    public void replyToIoTCore(Message request, Message reply)
    {
        messagingProvider.replyToIoTCore(request, reply);
    }

    /**
     * Unsubscribes from messages on a topic.
     *
     * @param topicFilter The topic filter to unsubscribe from
     */
    public void unsubscribe(String topicFilter)
    {
        messagingProvider.unsubscribe(topicFilter);
        LOGGER.debug("Unsubscribed to IPC messages on topic filter {}", topicFilter);
    }

    public void unsubscribeFromIoTCore(String topicFilter)
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
     * Returns the underlying native local messaging client implementation.
     *
     * @return The native messaging client object
     */
    public Object getNativeLocalClient()
    {
        return messagingProvider.getNativeLocalClient();
    }

    /**
     * Returns the underlying native iot core messaging client implementation.
     *
     * @return The native messaging client object
     */
    public Object getNativeIotCoreClient()
    {
        return messagingProvider.getNativeIotCoreClient();
    }

}