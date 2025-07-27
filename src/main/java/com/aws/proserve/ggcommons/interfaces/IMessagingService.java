/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.interfaces;

import com.aws.proserve.ggcommons.messaging.Message;
import com.aws.proserve.ggcommons.messaging.MessageHandler;
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
}