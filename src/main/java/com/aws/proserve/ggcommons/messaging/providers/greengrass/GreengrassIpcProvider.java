package com.aws.proserve.ggcommons.messaging.providers.greengrass;

import com.aws.proserve.ggcommons.messaging.Message;
import com.aws.proserve.ggcommons.messaging.MessagingProvider;
import com.aws.proserve.ggcommons.messaging.ReplyFuture;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import software.amazon.awssdk.aws.greengrass.GreengrassCoreIPCClientV2;
import software.amazon.awssdk.aws.greengrass.SubscribeToIoTCoreResponseHandler;
import software.amazon.awssdk.aws.greengrass.SubscribeToTopicResponseHandler;
import software.amazon.awssdk.aws.greengrass.model.*;
import java.nio.charset.StandardCharsets;
import java.util.HashMap;
import java.util.function.BiConsumer;

public class GreengrassIpcProvider extends MessagingProvider
{
    protected static final Logger LOGGER = LogManager.getLogger(GreengrassIpcProvider.class);
    GreengrassCoreIPCClientV2 ipcClient;
    HashMap<String, SubscribeToTopicResponseHandler> ipcSubscriptionStreams;
    HashMap<String, SubscribeToIoTCoreResponseHandler> iotCoreSubscriptionStreams;

    HashMap<String, ReplyFuture> responseFutures = new HashMap<>();

    final ReceiveMode receiveMode;

    public GreengrassIpcProvider(String[] messagingArgs, boolean receiveOwnMessages)
    {
        super(messagingArgs);
        receiveMode = receiveOwnMessages ? ReceiveMode.RECEIVE_ALL_MESSAGES : ReceiveMode.RECEIVE_MESSAGES_FROM_OTHERS;
        try
        {
            ipcClient = GreengrassCoreIPCClientV2.builder().build();
            ipcSubscriptionStreams = new HashMap<>();
            iotCoreSubscriptionStreams = new HashMap<>();
        }
        catch (Exception e)
        {
            LOGGER.fatal("Unable to connect to Greengrass IPC.");
            System.exit(5);
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
    public void subscribe(String topicFilter, BiConsumer<String, Message> callback, boolean serializeProcessing)
    {
        try
        {
            SubscribeToTopicRequest subRequest = new SubscribeToTopicRequest().withTopic(topicFilter).withReceiveMode(receiveMode);
            GreengrassCoreIPCClientV2.StreamingResponse<SubscribeToTopicResponse,
                    SubscribeToTopicResponseHandler> response =
                    ipcClient.subscribeToTopic(subRequest, new IpcSubscriptionHandler(topicFilter, callback, serializeProcessing));
            ipcSubscriptionStreams.put(topicFilter, response.getHandler());
        }
        catch (Exception e)
        {
            LOGGER.error("Failed to subscribe to IPC messages on topic filter {}", topicFilter);
        }
    }

    @Override
    public void subscribeToIoTCore(String topicFilter, BiConsumer<String, Message> callback, QOS qos,
                                   boolean serializeProcessing)
    {
        try
        {
            SubscribeToIoTCoreRequest subRequest = new SubscribeToIoTCoreRequest()
                    .withTopicName(topicFilter)
                    .withQos(qos);
            GreengrassCoreIPCClientV2.StreamingResponse<SubscribeToIoTCoreResponse,
                    SubscribeToIoTCoreResponseHandler> response =
                    ipcClient.subscribeToIoTCore(subRequest, new IotCoreSubscriptionHandler(topicFilter, callback, serializeProcessing));
            iotCoreSubscriptionStreams.put(topicFilter, response.getHandler());
        }
        catch (Exception e)
        {
            LOGGER.error("Failed to subscribe to IoT Core messages on topic filter {}: {}", topicFilter, e);
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
            f.complete(m);
            unsubscribe(t);
            responseFutures.remove(t);
        }, false);
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
}
