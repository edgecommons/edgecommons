/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.interfaces;

import com.aws.proserve.ggcommons.messaging.Message;
import com.aws.proserve.ggcommons.messaging.MessageHandler;
import com.aws.proserve.ggcommons.messaging.ReplyFuture;
import com.google.gson.JsonObject;
import software.amazon.awssdk.aws.greengrass.model.QOS;
import java.util.concurrent.CompletableFuture;

/**
 * Interface for messaging services.
 * Provides abstraction for different messaging providers (IPC, MQTT).
 */
public interface IMessagingService {
    
    /**
     * Subscribes to IPC messages on the specified topic.
     * 
     * @param topic Topic pattern (supports wildcards)
     * @param handler Message handler function
     * @param maxMessages Maximum concurrent messages
     */
    void subscribe(String topic, MessageHandler handler, int maxMessages);
    
    /**
     * Subscribes to IoT Core messages on the specified topic.
     * 
     * @param topic Topic pattern
     * @param handler Message handler function
     * @param qos Quality of service level
     */
    void subscribeToIoTCore(String topic, MessageHandler handler, QOS qos);
    
    /**
     * Subscribes to IoT Core messages with concurrency control.
     * 
     * @param topic Topic pattern
     * @param handler Message handler function
     * @param qos Quality of service level
     * @param maxMessages Maximum concurrent messages
     */
    void subscribeToIoTCore(String topic, MessageHandler handler, QOS qos, int maxMessages);
    
    /**
     * Publishes message via IPC.
     * 
     * @param topic Target topic
     * @param message Message to publish
     */
    void publish(String topic, Message message);
    
    /**
     * Publishes message to IoT Core.
     * 
     * @param topic Target topic
     * @param message Message to publish
     * @param qos Quality of service level
     */
    void publishToIotCore(String topic, Message message, QOS qos);
    
    /**
     * Publishes raw JSON payload via IPC.
     * 
     * @param topic Target topic
     * @param payload JSON payload to publish
     */
    void publishRaw(String topic, JsonObject payload);
    
    /**
     * Sends request via IPC and returns future for response.
     * 
     * @param topic Request topic
     * @param message Request message
     * @return CompletableFuture for the response
     */
    CompletableFuture<Message> request(String topic, Message message);
    
    /**
     * Sends request via IoT Core and returns future for response.
     * 
     * @param topic Request topic
     * @param message Request message
     * @return CompletableFuture for the response
     */
    CompletableFuture<Message> requestFromIoTCore(String topic, Message message);
    
    /**
     * Sends reply to a received message.
     * 
     * @param originalMessage The original message to reply to
     * @param replyMessage The reply message
     */
    void reply(Message originalMessage, Message replyMessage);
    
    /**
     * Subscribes to IPC messages without maxMessages parameter.
     * 
     * @param topicFilter Topic pattern (supports wildcards)
     * @param handler Message handler function
     */
    void subscribe(String topicFilter, MessageHandler handler);
    
    /**
     * Publishes raw JSON payload to IoT Core.
     * 
     * @param topic Target topic
     * @param payload JSON payload to publish
     * @param qos Quality of service level
     */
    void publishToIotCoreRaw(String topic, JsonObject payload, QOS qos);
    
    /**
     * Unsubscribes from IPC messages on a topic.
     * 
     * @param topicFilter The topic filter to unsubscribe from
     */
    void unsubscribe(String topicFilter);
    
    /**
     * Unsubscribes from IoT Core messages on a topic.
     * 
     * @param topicFilter The topic filter to unsubscribe from
     */
    void unsubscribeFromIoTCore(String topicFilter);
    
    /**
     * Cancels a pending IPC request.
     * 
     * @param replyFuture The ReplyFuture to cancel
     */
    void cancelRequest(ReplyFuture replyFuture);
    
    /**
     * Cancels a pending IoT Core request.
     * 
     * @param replyFuture The ReplyFuture to cancel
     */
    void cancelRequestFromIoTCore(ReplyFuture replyFuture);
    
    /**
     * Sends reply to an IoT Core message.
     * 
     * @param request The original request message
     * @param reply The reply message
     */
    void replyToIoTCore(Message request, Message reply);
    
    /**
     * Checks if a topic matches a topic filter pattern.
     * 
     * @param topicFilter The topic filter pattern
     * @param topic The topic to check
     * @return true if the topic matches the filter
     */
    boolean topicMatchesFilter(String topicFilter, String topic);
    
    /**
     * Returns the native local messaging client.
     * 
     * @return The native messaging client object
     */
    Object getNativeLocalClient();
    
    /**
     * Returns the native IoT Core messaging client.
     * 
     * @return The native messaging client object
     */
    Object getNativeIotCoreClient();
}