/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.messaging.providers.greengrass;

import com.mbreissi.edgecommons.messaging.Message;
import com.mbreissi.edgecommons.messaging.MessagingProvider;
import com.mbreissi.edgecommons.messaging.PublishConfirmationException;
import com.mbreissi.edgecommons.messaging.Qos;
import com.mbreissi.edgecommons.messaging.ReplyFuture;
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
import java.time.Duration;
import java.util.Map;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.concurrent.Semaphore;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.TimeoutException;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.function.BiConsumer;
import java.util.function.Supplier;

public final class GreengrassMessagingProvider extends MessagingProvider
{
    protected static final Logger LOGGER = LogManager.getLogger(GreengrassMessagingProvider.class);
    GreengrassCoreIPCClientV2 ipcClient;
    ConcurrentHashMap<String, SubscribeToTopicResponseHandler> ipcSubscriptionStreams;
    ConcurrentHashMap<String, SubscribeToIoTCoreResponseHandler> iotCoreSubscriptionStreams;

    ConcurrentHashMap<String, ReplyFuture> responseFutures = new ConcurrentHashMap<>();

    final ReceiveMode receiveMode;

    /** Bounds IPC operations concurrently waiting for successful operation completion. */
    private final Semaphore confirmedPublishPermits = new Semaphore(
            DEFAULT_MAX_IN_FLIGHT_CONFIRMED_PUBLISHES);

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

    private static QOS toGreengrassQos(Qos qos)
    {
        return switch (qos) {
            case AT_MOST_ONCE -> QOS.AT_MOST_ONCE;
            case AT_LEAST_ONCE -> QOS.AT_LEAST_ONCE;
            case EXACTLY_ONCE -> throw new IllegalArgumentException(
                    "Greengrass IoT Core IPC supports only MQTT QoS 0 and 1; got EXACTLY_ONCE");
        };
    }

    @Override
    public void close()
    {
        super.close();  // shuts down the shared request-deadline scheduler
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
            BinaryMessage ipcMessage = new BinaryMessage().withMessage(message.toBytes());
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
    public void publishNorthbound(String topic, Message message, Qos qos)
    {
        try
        {
            PublishToIoTCoreRequest pubRequest = new PublishToIoTCoreRequest()
                    .withTopicName(topic)
                    .withPayload(message.toBytes())
                    .withQos(toGreengrassQos(qos));
            ipcClient.publishToIoTCore(pubRequest);
        }
        catch (InterruptedException e)
        {
            LOGGER.error("Failed to publish IPC message on topic {}", topic);
        }
    }

    @Override
    public void publishConfirmed(String topic, byte[] encodedMessage, Qos qos, Duration timeout)
    {
        confirmedTimeoutMillis(encodedMessage, qos, timeout);
        BinaryMessage ipcMessage = new BinaryMessage().withMessage(encodedMessage.clone());
        PublishMessage publishMessage = new PublishMessage().withBinaryMessage(ipcMessage);
        PublishToTopicRequest request = new PublishToTopicRequest()
                .withTopic(topic)
                .withPublishMessage(publishMessage);
        awaitConfirmed(topic, timeout, () -> ipcClient.publishToTopicAsync(request));
    }

    @Override
    public void publishNorthboundConfirmed(String topic, byte[] encodedMessage, Qos qos,
                                            Duration timeout)
    {
        confirmedTimeoutMillis(encodedMessage, qos, timeout);
        PublishToIoTCoreRequest request = new PublishToIoTCoreRequest()
                .withTopicName(topic)
                .withPayload(encodedMessage.clone())
                .withQos(QOS.AT_LEAST_ONCE);
        awaitConfirmed(topic, timeout, () -> ipcClient.publishToIoTCoreAsync(request));
    }

    /** Waits for the Greengrass IPC operation response; scheduling the operation is not success. */
    private void awaitConfirmed(String topic, Duration timeout,
                                Supplier<CompletableFuture<?>> operationFactory)
    {
        long timeoutMillis = timeout.toMillis();
        long timeoutNanos = TimeUnit.MILLISECONDS.toNanos(timeoutMillis);
        long started = System.nanoTime();
        boolean acquired = false;
        CompletableFuture<?> operation = null;
        try
        {
            acquired = confirmedPublishPermits.tryAcquire(timeoutNanos, TimeUnit.NANOSECONDS);
            if (!acquired)
            {
                throw confirmationFailure(PublishConfirmationException.Reason.TIMEOUT, topic,
                        "timed out waiting for confirmed-publish capacity", null);
            }
            long remaining = timeoutNanos - (System.nanoTime() - started);
            if (remaining <= 0)
            {
                throw confirmationFailure(PublishConfirmationException.Reason.TIMEOUT, topic,
                        "IPC confirmation timeout elapsed before send", null);
            }
            operation = operationFactory.get();
            remaining = timeoutNanos - (System.nanoTime() - started);
            if (remaining <= 0)
            {
                operation.cancel(true);
                throw confirmationFailure(PublishConfirmationException.Reason.TIMEOUT, topic,
                        "IPC confirmation timeout elapsed while starting the operation", null);
            }
            operation.get(remaining, TimeUnit.NANOSECONDS);
        }
        catch (TimeoutException e)
        {
            if (operation != null)
            {
                operation.cancel(true);
            }
            throw confirmationFailure(PublishConfirmationException.Reason.TIMEOUT, topic,
                    "IPC publish operation did not complete before timeout", e);
        }
        catch (InterruptedException e)
        {
            if (operation != null)
            {
                operation.cancel(true);
            }
            Thread.currentThread().interrupt();
            throw confirmationFailure(PublishConfirmationException.Reason.INTERRUPTED, topic,
                    "interrupted before IPC publish confirmation", e);
        }
        catch (ExecutionException | java.util.concurrent.CancellationException e)
        {
            Throwable cause = e instanceof ExecutionException && e.getCause() != null
                    ? e.getCause() : e;
            throw confirmationFailure(PublishConfirmationException.Reason.TRANSPORT_ERROR, topic,
                    "IPC publish operation failed", cause);
        }
        catch (PublishConfirmationException e)
        {
            throw e;
        }
        catch (RuntimeException e)
        {
            throw confirmationFailure(PublishConfirmationException.Reason.TRANSPORT_ERROR, topic,
                    "IPC publish operation could not be started", e);
        }
        finally
        {
            if (acquired)
            {
                confirmedPublishPermits.release();
            }
        }
    }

    private static PublishConfirmationException confirmationFailure(
            PublishConfirmationException.Reason reason, String topic, String detail,
            Throwable cause)
    {
        return new PublishConfirmationException(reason,
                "confirmed publish on '" + topic + "' failed: " + detail, cause);
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
    public void publishNorthboundRaw(String topic, JsonObject payload, Qos qos)
    {
        try
        {
            PublishToIoTCoreRequest pubRequest = new PublishToIoTCoreRequest()
                    .withTopicName(topic)
                    .withPayload(payload.toString().getBytes(StandardCharsets.UTF_8))
                    .withQos(toGreengrassQos(qos));
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

    @Override
    public void subscribeAcknowledged(String topicFilter,
                                      BiConsumer<String, Message> callback,
                                      int maxConcurrency,
                                      int maxMessages,
                                      Duration timeout)
    {
        long timeoutMillis = subscriptionTimeoutMillis(timeout);
        AtomicBoolean abandoned = new AtomicBoolean(false);
        ExecutorService waiter = Executors.newVirtualThreadPerTaskExecutor();
        Future<GreengrassCoreIPCClientV2.StreamingResponse<SubscribeToTopicResponse,
                SubscribeToTopicResponseHandler>> operation = waiter.submit(() -> {
            SubscribeToTopicRequest request = new SubscribeToTopicRequest()
                    .withTopic(topicFilter)
                    .withReceiveMode(receiveMode);
            GreengrassCoreIPCClientV2.StreamingResponse<SubscribeToTopicResponse,
                    SubscribeToTopicResponseHandler> response = ipcClient.subscribeToTopic(
                    request,
                    new IpcSubscriptionHandler(
                            topicFilter, callback, maxConcurrency, maxMessages));
            if (abandoned.get())
            {
                response.getHandler().closeStream();
            }
            return response;
        });
        try
        {
            GreengrassCoreIPCClientV2.StreamingResponse<SubscribeToTopicResponse,
                    SubscribeToTopicResponseHandler> response =
                    operation.get(timeoutMillis, TimeUnit.MILLISECONDS);
            if (abandoned.get())
            {
                response.getHandler().closeStream();
                throw new IllegalStateException(
                        "Greengrass subscription completed after its deadline");
            }
            SubscribeToTopicResponseHandler existing = ipcSubscriptionStreams.putIfAbsent(
                    topicFilter, response.getHandler());
            if (existing != null)
            {
                response.getHandler().closeStream();
                throw new IllegalStateException(
                        "an IPC subscription already exists for '" + topicFilter + "'");
            }
        }
        catch (TimeoutException e)
        {
            abandoned.set(true);
            operation.cancel(true);
            throw new IllegalStateException(
                    "Greengrass subscription operation did not complete before its deadline", e);
        }
        catch (InterruptedException e)
        {
            abandoned.set(true);
            operation.cancel(true);
            Thread.currentThread().interrupt();
            throw new IllegalStateException(
                    "interrupted while waiting for Greengrass subscription completion", e);
        }
        catch (ExecutionException e)
        {
            abandoned.set(true);
            Throwable cause = e.getCause() == null ? e : e.getCause();
            throw new IllegalStateException(
                    "Greengrass subscription operation failed", cause);
        }
        finally
        {
            waiter.shutdownNow();
        }
    }

    public void subscribeNorthbound(String topicFilter, BiConsumer<String, Message> callback, Qos qos,
                                   int maxConcurrency, int maxMessages)
    {
        try
        {
            SubscribeToIoTCoreRequest subRequest = new SubscribeToIoTCoreRequest()
                    .withTopicName(topicFilter)
                    .withQos(toGreengrassQos(qos));
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
    public void unsubscribeNorthbound(String topicFilter)
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
        return request(topic, message, null);
    }

    @Override
    public ReplyFuture request(String topic, Message message, Duration timeout)
    {
        String replyTo = message.makeRequest();
        ReplyFuture future = new ReplyFuture(replyTo);
        responseFutures.put(replyTo, future);
        subscribe(replyTo, (t, m) -> {
            // Reply arrival: race the single idempotent settle path (§5.1) against the framework
            // deadline and cancelRequest. The winner owns the cleanup and the completion; a loser
            // (straggler / duplicate reply after settle) is dropped at DEBUG — never a double
            // unsubscribe or double completion.
            ReplyFuture f = responseFutures.get(t);
            if (f == null || !f.trySettle()) {
                LOGGER.debug("Dropping straggler reply on '{}' (request already settled)", t);
                return;
            }
            unsubscribe(t);
            responseFutures.remove(t);
            f.complete(m);
        }, 1, -1); // one-shot reply sub: unbounded is fine (exactly one reply, then unsubscribe)
        // Arm the framework-owned deadline at send time (§5): on expiry the timer unsubscribes the
        // ephemeral reply topic, removes the pending entry and completes the future exceptionally
        // (TimeoutException) — even when the caller never awaits the future.
        armRequestDeadline(future, effectiveRequestTimeout(timeout), () -> {
            unsubscribe(replyTo);
            responseFutures.remove(replyTo);
        });
        publish(topic, message);
        return future;
    }

    @Override
    public void cancelRequest(ReplyFuture future)
    {
        if (!future.trySettle())
        {
            return;  // reply or deadline already settled + cleaned up this request
        }
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
    public ReplyFuture requestNorthbound(String topic, Message request)
    {
        return requestNorthbound(topic, request, null);
    }

    @Override
    public ReplyFuture requestNorthbound(String topic, Message request, Duration timeout)
    {
        String replyTo = request.makeRequest();
        ReplyFuture future = new ReplyFuture(replyTo);
        responseFutures.put(replyTo, future);
        subscribeNorthbound(replyTo, (t, m) -> {
            // Same single idempotent settle path as request() (§5.1).
            ReplyFuture f = responseFutures.get(t);
            if (f == null || !f.trySettle()) {
                LOGGER.debug("Dropping straggler reply on '{}' (request already settled)", t);
                return;
            }
            unsubscribeNorthbound(t);
            responseFutures.remove(t);
            f.complete(m);
        }, Qos.AT_MOST_ONCE, 1, -1); // one-shot reply sub: unbounded is fine
        armRequestDeadline(future, effectiveRequestTimeout(timeout), () -> {
            unsubscribeNorthbound(replyTo);
            responseFutures.remove(replyTo);
        });
        publishNorthbound(topic, request, Qos.AT_MOST_ONCE);
        return future;
    }

    @Override
    public void cancelRequestNorthbound(ReplyFuture future)
    {
        if (!future.trySettle())
        {
            return;  // reply or deadline already settled + cleaned up this request
        }
        unsubscribeNorthbound(future.replyTopic);
        responseFutures.remove(future.replyTopic);
        future.complete(null);
    }

    @Override
    public void replyNorthbound(Message request, Message reply)
    {
        reply.setCorrelationId(request.getHeader().getCorrelationId());
        publishNorthbound(request.getHeader().getReplyTo(), reply, Qos.AT_MOST_ONCE);
    }

    @Override
    public Object getNativeLocalClient()
    {
        return ipcClient;
    }

    @Override
    public Object getNativeNorthboundClient()
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
