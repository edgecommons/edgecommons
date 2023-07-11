package com.aws.proserve.ggcommons.messaging.providers;

import com.aws.proserve.ggcommons.messaging.Message;
import com.aws.proserve.ggcommons.messaging.MessagingProvider;
import com.github.cliftonlabs.json_simple.JsonObject;
import com.github.cliftonlabs.json_simple.Jsoner;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import software.amazon.awssdk.aws.greengrass.GreengrassCoreIPCClientV2;
import software.amazon.awssdk.aws.greengrass.SubscribeToTopicResponseHandler;
import software.amazon.awssdk.aws.greengrass.model.*;
import software.amazon.awssdk.eventstreamrpc.StreamResponseHandler;

import java.nio.charset.StandardCharsets;
import java.util.HashMap;
import java.util.function.BiConsumer;

public class GreengrassIpcProvider extends MessagingProvider
{
    protected static final Logger LOGGER = LogManager.getLogger(GreengrassIpcProvider.class);
    GreengrassCoreIPCClientV2 ipcClient;
    HashMap<String, SubscribeToTopicResponseHandler> subscriptionStreams;

    final ReceiveMode receiveMode;

    private static class MessageHandler implements StreamResponseHandler<SubscriptionResponseMessage>
    {
        private final String topicFilter;
        private final BiConsumer<String, Message> callback;

        public MessageHandler(String topicFilter, BiConsumer<String, Message> callback)
        {
            this.topicFilter = topicFilter;
            this.callback = callback;
        }

        @Override
        public void onStreamEvent(SubscriptionResponseMessage subscriptionResponseMessage)
        {
            LOGGER.debug("IPC Message received on subscription to topic filter '{}'", topicFilter);
            String topic;
            try
            {
                JsonObject receivedPayload;
                if (subscriptionResponseMessage.getJsonMessage() != null)
                {
                    receivedPayload = new JsonObject(subscriptionResponseMessage.getJsonMessage().getMessage());
                    LOGGER.trace("Received json message: {} of type {}", receivedPayload.toJson(), receivedPayload.getClass().getSimpleName());
                    topic = subscriptionResponseMessage.getJsonMessage().getContext().getTopic();
                    LOGGER.trace("On topic {}", topic);
                }
                else
                {
                    String decodedBinaryPayload = new String(subscriptionResponseMessage.getBinaryMessage().getMessage(), StandardCharsets.UTF_8);
                    LOGGER.trace("Received binary message: {}", decodedBinaryPayload);
                    receivedPayload = (JsonObject) Jsoner.deserialize(decodedBinaryPayload);
                    topic = subscriptionResponseMessage.getBinaryMessage().getContext().getTopic();
                }
                LOGGER.trace("Invoking callback");
                callback.accept(topic, Message.build(receivedPayload));
            }
            catch (Exception e)
            {
                LOGGER.error("Problem decoding IPC payload into Message on topic {}: {}", topicFilter, e.toString());
            }
        }

        @Override
        public boolean onStreamError(Throwable throwable)
        {
            LOGGER.error("Error on IPC stream for subscription to topicFilter {}: {}", topicFilter, throwable.toString());
            return false;
        }

        @Override
        public void onStreamClosed()
        {
            LOGGER.info("IPC stream for subscription to topicFilter {} closed (unsubscribed)", topicFilter);
        }
    }
    public GreengrassIpcProvider(String[] messagingArgs, boolean receiveOwnMessages)
    {
        super(messagingArgs);
        receiveMode = receiveOwnMessages ? ReceiveMode.RECEIVE_ALL_MESSAGES : ReceiveMode.RECEIVE_MESSAGES_FROM_OTHERS;
        try
        {
            ipcClient = GreengrassCoreIPCClientV2.builder().build();
            subscriptionStreams = new HashMap<>();
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
//            JsonMessage ipcMessage = new JsonMessage().withMessage(message.toDict());
//            PublishMessage pubMessage = new PublishMessage().withJsonMessage(ipcMessage);
//            PublishToTopicRequest pubRequest = new PublishToTopicRequest().withTopic(topic).withPublishMessage(pubMessage);
//            ipcClient.publishToTopic(pubRequest);
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

    // function that uses greengrass ipc client v2  to fetch a named shadow document

    @Override
    public void subscribe(String topicFilter, BiConsumer<String, Message> callback)
    {
        try
        {
            SubscribeToTopicRequest subRequest = new SubscribeToTopicRequest().withTopic(topicFilter).withReceiveMode(receiveMode);
            GreengrassCoreIPCClientV2.StreamingResponse<SubscribeToTopicResponse,
                    SubscribeToTopicResponseHandler> response =
                    ipcClient.subscribeToTopic(subRequest, new MessageHandler(topicFilter, callback));
            subscriptionStreams.put(topicFilter, response.getHandler());
        }
        catch (Exception e)
        {
            LOGGER.error("Failed to subscribe to IPC messages on topic filter {}", topicFilter);
        }
    }

    @Override
    public void unsubscribe(String topicFilter)
    {
        SubscribeToTopicResponseHandler responseHandler = subscriptionStreams.getOrDefault(topicFilter, null);
        if (responseHandler != null)
        {
            responseHandler.closeStream();
            subscriptionStreams.remove(topicFilter);
        }
        LOGGER.debug("Unsubscribed from IPC messages on topic filter {}", topicFilter);
    }
}
