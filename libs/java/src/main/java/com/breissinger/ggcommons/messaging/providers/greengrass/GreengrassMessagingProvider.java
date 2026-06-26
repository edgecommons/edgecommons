/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.breissinger.ggcommons.messaging.providers.greengrass;

import com.breissinger.ggcommons.messaging.Message;
import com.breissinger.ggcommons.messaging.MessagingProvider;
import com.breissinger.ggcommons.messaging.ReplyFuture;
import com.google.gson.Gson;
import com.google.gson.JsonObject;
import com.google.gson.reflect.TypeToken;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import software.amazon.awssdk.aws.greengrass.GreengrassCoreIPCClientV2;
import software.amazon.awssdk.aws.greengrass.SubscribeToIoTCoreResponseHandler;
import software.amazon.awssdk.aws.greengrass.SubscribeToTopicResponseHandler;
import software.amazon.awssdk.aws.greengrass.model.*;

import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.function.BiConsumer;

public final class GreengrassMessagingProvider extends MessagingProvider
{
    protected static final Logger LOGGER = LogManager.getLogger(GreengrassMessagingProvider.class);
    GreengrassCoreIPCClientV2 ipcClient;
    ConcurrentHashMap<String, SubscribeToTopicResponseHandler> ipcSubscriptionStreams;
    ConcurrentHashMap<String, SubscribeToIoTCoreResponseHandler> iotCoreSubscriptionStreams;

    ConcurrentHashMap<String, ReplyFuture> responseFutures = new ConcurrentHashMap<>();

    final ReceiveMode receiveMode;

    public GreengrassMessagingProvider(boolean receiveOwnMessages)
    {
        receiveMode = receiveOwnMessages ? ReceiveMode.RECEIVE_ALL_MESSAGES : ReceiveMode.RECEIVE_MESSAGES_FROM_OTHERS;
        try
        {
            ipcClient = GreengrassCoreIPCClientV2.builder().build();
            ipcSubscriptionStreams = new ConcurrentHashMap<>();
            iotCoreSubscriptionStreams = new ConcurrentHashMap<>();
        }
        catch (IOException e)
        {
            LOGGER.fatal("Unable to connect to Greengrass IPC due to I/O error.", e);
            throw new RuntimeException("Unable to connect to Greengrass IPC due to I/O error.", e);
        }
    }

    @Override
    public void close()
    {
        try
        {
            if (ipcClient != null)
            {
                ipcClient.close();
            }
        }
        catch (Exception e)
        {
            LOGGER.warn("Error closing Greengrass IPC client: {}", e.getMessage());
        }
    }

    @Override
    public void publish(String topic, Message message)
    {
        try
        {
            BinaryMessage ipcMessage = new BinaryMessage().withMessage(message.toString().getBytes(StandardCharsets.UTF_8));
            PublishMessage pubMessage = new PublishMessage().withBinaryMessage(ipcMessage);
            PublishToTopicRequest pubRequest = new PublishToTopicRequest().withTopic(topic).withPublishMessage(pubMessage);
            ipcClient.publishToTopic(pubRequest);
        }
        catch (InterruptedException e)
        {
            LOGGER.error("Failed to publish IPC message on topic {}", topic);
        }
    }

    @Override
    public void publishToIoTCore(String topic, Message message, QOS qos)
    {
        try
        {
            PublishToIoTCoreRequest pubRequest = new PublishToIoTCoreRequest()
                    .withTopicName(topic)
                    .withPayload(message.toString().getBytes(StandardCharsets.UTF_8))
                    .withQos(qos);
            ipcClient.publishToIoTCore(pubRequest);
        }
        catch (InterruptedException e)
        {
            LOGGER.error("Failed to publish IPC message on topic {}", topic);
        }
    }

    @Override
    public void publishRaw(String topic, JsonObject payload)
    {
        try
        {
            Gson gson = new Gson();
            TypeToken<Map<String, Object>> typeToken = new TypeToken<>() {};
            Map<String, Object> map = gson.fromJson(payload, typeToken.getType());
            JsonMessage ipcMessage = new JsonMessage().withMessage(map);
            PublishMessage pubMessage = new PublishMessage().withJsonMessage(ipcMessage);
            PublishToTopicRequest pubRequest = new PublishToTopicRequest().withTopic(topic).withPublishMessage(pubMessage);
            ipcClient.publishToTopic(pubRequest);
        }
        catch (InterruptedException e)
        {
            LOGGER.error("Failed to publish IPC message on topic {}", topic);
        }
    }

    @Override
    public void publishToIoTCoreRaw(String topic, JsonObject payload, QOS qos)
    {
        try
        {
            PublishToIoTCoreRequest pubRequest = new PublishToIoTCoreRequest()
                    .withTopicName(topic)
                    .withPayload(payload.toString().getBytes(StandardCharsets.UTF_8))
                    .withQos(qos);
            ipcClient.publishToIoTCore(pubRequest);
        }
        catch (InterruptedException e)
        {
            LOGGER.error("Failed to publish IPC message on topic {}", topic);
        }
    }


    @Override
    public void subscribe(String topicFilter, BiConsumer<String, Message> callback, int maxConcurrency, int maxMessages)
    {
        try
        {
            SubscribeToTopicRequest subRequest = new SubscribeToTopicRequest().withTopic(topicFilter).withReceiveMode(receiveMode);
            GreengrassCoreIPCClientV2.StreamingResponse<SubscribeToTopicResponse,
                    SubscribeToTopicResponseHandler> response =
                    ipcClient.subscribeToTopic(subRequest, new IpcSubscriptionHandler(topicFilter, callback, maxConcurrency, maxMessages));
            ipcSubscriptionStreams.put(topicFilter, response.getHandler());
        }
        catch (Exception e)
        {
            LOGGER.error("Failed to subscribe to IPC messages on topic filter {}", topicFilter);
        }
    }

    public void subscribeToIoTCore(String topicFilter, BiConsumer<String, Message> callback, QOS qos,
                                   int maxConcurrency, int maxMessages)
    {
        try
        {
            SubscribeToIoTCoreRequest subRequest = new SubscribeToIoTCoreRequest()
                    .withTopicName(topicFilter)
                    .withQos(qos);
            GreengrassCoreIPCClientV2.StreamingResponse<SubscribeToIoTCoreResponse,
                    SubscribeToIoTCoreResponseHandler> response =
                    ipcClient.subscribeToIoTCore(subRequest, new IotCoreSubscriptionHandler(topicFilter, callback, maxConcurrency, maxMessages));
            iotCoreSubscriptionStreams.put(topicFilter, response.getHandler());
        }
        catch (InterruptedException e) // import java.lang.InterruptedException
        {
            LOGGER.error("Thread interrupted while subscribing to IoT Core messages on topic filter {}: {}", topicFilter, e);
        }
        catch (Exception e)
        {
            LOGGER.error("Unexpected error occurred while subscribing to IoT Core messages on topic filter {}: {}", topicFilter, e);
        }
    }

    @Override
    public void unsubscribe(String topicFilter)
    {
        SubscribeToTopicResponseHandler responseHandler = ipcSubscriptionStreams.getOrDefault(topicFilter, null);
        if (responseHandler != null)
        {
            responseHandler.closeStream();
            ipcSubscriptionStreams.remove(topicFilter);
        }
        LOGGER.debug("Unsubscribed from IPC messages on topic filter {}", topicFilter);
    }

    @Override
    public void unsubscribeFromIoTCore(String topicFilter)
    {
        SubscribeToIoTCoreResponseHandler responseHandler = iotCoreSubscriptionStreams.getOrDefault(topicFilter, null);
        if (responseHandler != null)
        {
            responseHandler.closeStream();
            iotCoreSubscriptionStreams.remove(topicFilter);
        }
        LOGGER.debug("Unsubscribed from IoT Core messages on topic filter {}", topicFilter);
    }

    @Override
    public ReplyFuture request(String topic, Message message)
    {
        String replyTo = message.makeRequest();
        ReplyFuture future = new ReplyFuture(replyTo);
        responseFutures.put(replyTo, future);
        subscribe(replyTo, (t, m) -> {
            ReplyFuture f = responseFutures.get(t);
            // A late or duplicate reply can arrive after the future was completed + removed;
            // guard against the NPE (f.complete on null) — it must never escape this callback.
            if (f != null) {
                f.complete(m);
            }
            unsubscribe(t);
            responseFutures.remove(t);
        }, 1, -1); // one-shot reply sub: unbounded is fine (exactly one reply, then unsubscribe)
        publish(topic, message);
        return future;
    }

    @Override
    public void cancelRequest(ReplyFuture future)
    {
        unsubscribe(future.replyTopic);
        responseFutures.remove(future.replyTopic);
        future.complete(null);
    }

    @Override
    public void reply(Message request, Message reply)
    {
        reply.setCorrelationId(request.getHeader().getCorrelationId());
        publish(request.getHeader().getReplyTo(), reply);
    }

    @Override
    public ReplyFuture requestFromIoTCore(String topic, Message request)
    {
        String replyTo = request.makeRequest();
        ReplyFuture future = new ReplyFuture(replyTo);
        responseFutures.put(replyTo, future);
        subscribeToIoTCore(replyTo, (t, m) -> {
            ReplyFuture f = responseFutures.get(t);
            // Guard against a late/duplicate reply after the future was completed + removed.
            if (f != null) {
                f.complete(m);
            }
            unsubscribeFromIoTCore(t);
            responseFutures.remove(t);
        }, QOS.AT_MOST_ONCE, 1, -1); // one-shot reply sub: unbounded is fine
        publishToIoTCore(topic, request, QOS.AT_MOST_ONCE);
        return future;
    }

    @Override
    public void cancelRequestFromIoTCore(ReplyFuture future)
    {
        unsubscribeFromIoTCore(future.replyTopic);
        responseFutures.remove(future.replyTopic);
        future.complete(null);
    }

    @Override
    public void replyToIoTCore(Message request, Message reply)
    {
        reply.setCorrelationId(request.getHeader().getCorrelationId());
        publishToIoTCore(request.getHeader().getReplyTo(), reply, QOS.AT_MOST_ONCE);
    }

    @Override
    public Object getNativeLocalClient()
    {
        return ipcClient;
    }

    @Override
    public Object getNativeIotCoreClient()
    {
        return ipcClient;
    }

    /**
     * Reports IPC connectivity for the readiness model (FR-HB-2): {@code true} once the Greengrass IPC
     * client has been built (the constructor connects to the Nucleus over the IPC domain socket, or
     * throws). There is no broker link to lose for IPC, so this stays {@code true} for the client's
     * lifetime.
     *
     * @return {@code true} when the IPC client is present
     */
    @Override
    public boolean connected()
    {
        return ipcClient != null;
    }
}
