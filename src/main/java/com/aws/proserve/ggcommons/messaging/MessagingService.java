/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.messaging;

import com.aws.proserve.ggcommons.interfaces.IMessagingService;
import com.google.gson.JsonObject;
import software.amazon.awssdk.aws.greengrass.model.QOS;
import java.util.concurrent.CompletableFuture;

/**
 * Service implementation that wraps MessagingClient to provide the IMessagingService interface.
 * This allows for dependency injection while maintaining backward compatibility.
 */
public class MessagingService implements IMessagingService {
    
    @Override
    public void subscribe(String topic, MessageHandler handler, int maxMessages) {
        MessagingClient.subscribe(topic, handler::handle, maxMessages);
    }
    
    @Override
    public void subscribeToIoTCore(String topic, MessageHandler handler, QOS qos) {
        MessagingClient.subscribeToIoTCore(topic, handler::handle, qos);
    }
    
    @Override
    public void subscribeToIoTCore(String topic, MessageHandler handler, QOS qos, int maxMessages) {
        MessagingClient.subscribeToIoTCore(topic, handler::handle, qos, maxMessages);
    }
    
    @Override
    public void publish(String topic, Message message) {
        MessagingClient.publish(topic, message);
    }
    
    @Override
    public void publishToIotCore(String topic, Message message, QOS qos) {
        MessagingClient.publishToIotCore(topic, message, qos);
    }
    
    @Override
    public void publishRaw(String topic, JsonObject payload) {
        MessagingClient.publishRaw(topic, payload);
    }
    
    @Override
    public CompletableFuture<Message> request(String topic, Message message) {
        return MessagingClient.request(topic, message);
    }
    
    @Override
    public CompletableFuture<Message> requestFromIoTCore(String topic, Message message) {
        return MessagingClient.requestFromIoTCore(topic, message);
    }
    
    @Override
    public void reply(Message originalMessage, Message replyMessage) {
        MessagingClient.reply(originalMessage, replyMessage);
    }
}