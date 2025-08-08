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
    
    private final MessagingClient messagingClient;
    
    public MessagingService(MessagingClient messagingClient) {
        this.messagingClient = messagingClient;
    }
    
    @Override
    public void subscribe(String topic, MessageHandler handler, int maxMessages) {
        messagingClient.subscribe(topic, handler::handle, maxMessages);
    }
    
    @Override
    public void subscribeToIoTCore(String topic, MessageHandler handler, QOS qos) {
        messagingClient.subscribeToIoTCore(topic, handler::handle, qos);
    }
    
    @Override
    public void subscribeToIoTCore(String topic, MessageHandler handler, QOS qos, int maxMessages) {
        messagingClient.subscribeToIoTCore(topic, handler::handle, qos, maxMessages);
    }
    
    @Override
    public void publish(String topic, Message message) {
        messagingClient.publish(topic, message);
    }
    
    @Override
    public void publishToIotCore(String topic, Message message, QOS qos) {
        messagingClient.publishToIotCore(topic, message, qos);
    }
    
    @Override
    public void publishRaw(String topic, JsonObject payload) {
        messagingClient.publishRaw(topic, payload);
    }
    
    @Override
    public CompletableFuture<Message> request(String topic, Message message) {
        return messagingClient.request(topic, message);
    }
    
    @Override
    public CompletableFuture<Message> requestFromIoTCore(String topic, Message message) {
        return messagingClient.requestFromIoTCore(topic, message);
    }
    
    @Override
    public void reply(Message originalMessage, Message replyMessage) {
        messagingClient.reply(originalMessage, replyMessage);
    }
    
    @Override
    public void subscribe(String topicFilter, MessageHandler handler) {
        messagingClient.subscribe(topicFilter, handler::handle);
    }
    
    @Override
    public void publishToIotCoreRaw(String topic, JsonObject payload, QOS qos) {
        messagingClient.publishToIotCoreRaw(topic, payload, qos);
    }
    
    @Override
    public void unsubscribe(String topicFilter) {
        messagingClient.unsubscribe(topicFilter);
    }
    
    @Override
    public void unsubscribeFromIoTCore(String topicFilter) {
        messagingClient.unsubscribeFromIoTCore(topicFilter);
    }
    
    @Override
    public void cancelRequest(ReplyFuture replyFuture) {
        messagingClient.cancelRequest(replyFuture);
    }
    
    @Override
    public void cancelRequestFromIoTCore(ReplyFuture replyFuture) {
        messagingClient.cancelRequestFromIoTCore(replyFuture);
    }
    
    @Override
    public void replyToIoTCore(Message request, Message reply) {
        messagingClient.replyToIoTCore(request, reply);
    }
    
    @Override
    public boolean topicMatchesFilter(String topicFilter, String topic) {
        return MessagingClient.topicMatchesFilter(topicFilter, topic);
    }
    
    @Override
    public Object getNativeLocalClient() {
        return messagingClient.getNativeLocalClient();
    }
    
    @Override
    public Object getNativeIotCoreClient() {
        return messagingClient.getNativeIotCoreClient();
    }
}