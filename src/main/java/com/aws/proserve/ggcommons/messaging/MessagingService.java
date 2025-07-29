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
    
    @Override
    public void subscribe(String topicFilter, MessageHandler handler) {
        MessagingClient.subscribe(topicFilter, handler::handle);
    }
    
    @Override
    public void publishToIotCoreRaw(String topic, JsonObject payload, QOS qos) {
        MessagingClient.publishToIotCoreRaw(topic, payload, qos);
    }
    
    @Override
    public void unsubscribe(String topicFilter) {
        MessagingClient.unsubscribe(topicFilter);
    }
    
    @Override
    public void unsubscribeFromIoTCore(String topicFilter) {
        MessagingClient.unsubscribeFromIoTCore(topicFilter);
    }
    
    @Override
    public void cancelRequest(ReplyFuture replyFuture) {
        MessagingClient.cancelRequest(replyFuture);
    }
    
    @Override
    public void cancelRequestFromIoTCore(ReplyFuture replyFuture) {
        MessagingClient.cancelRequestFromIoTCore(replyFuture);
    }
    
    @Override
    public void replyToIoTCore(Message request, Message reply) {
        MessagingClient.replyToIoTCore(request, reply);
    }
    
    @Override
    public boolean topicMatchesFilter(String topicFilter, String topic) {
        return MessagingClient.topicMatchesFilter(topicFilter, topic);
    }
    
    @Override
    public Object getNativeLocalClient() {
        return MessagingClient.getNativeLocalClient();
    }
    
    @Override
    public Object getNativeIotCoreClient() {
        return MessagingClient.getNativeIotCoreClient();
    }
}