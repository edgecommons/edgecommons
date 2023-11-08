package com.aws.proseve.ggcommons.messaging.providers.greengrass;


import com.github.cliftonlabs.json_simple.JsonObject;
import com.github.cliftonlabs.json_simple.Jsoner;
import com.aws.proseve.ggcommons.messaging.Message;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import oshi.util.tuples.Pair;
import software.amazon.awssdk.aws.greengrass.model.SubscriptionResponseMessage;

import java.nio.charset.StandardCharsets;
import java.util.function.BiConsumer;

public class IpcSubscriptionHandler extends SubscriptionHandler<SubscriptionResponseMessage>
{
    protected static final Logger LOGGER = LogManager.getLogger(IpcSubscriptionHandler.class);

    public IpcSubscriptionHandler(String topicFilter, BiConsumer<String, Message> callback, int maxConcurrency)
    {
        super(topicFilter, callback, maxConcurrency);
    }

    @Override
    Pair<String, Message> parseRawPayload(SubscriptionResponseMessage subscriptionResponseMessage)
    {
        LOGGER.debug("IPC Message received on subscription to topic filter '{}'", topicFilter);
        Pair<String, Message> retVal = null;
        try
        {
            String topic;
            JsonObject receivedPayload;
            if (subscriptionResponseMessage.getJsonMessage() != null)
            {
                receivedPayload = new JsonObject(subscriptionResponseMessage.getJsonMessage().getMessage());
                topic = subscriptionResponseMessage.getJsonMessage().getContext().getTopic();
                LOGGER.trace("Received json message: {} on topic {}", receivedPayload.toJson(), topic);
            }
            else
            {
                String decodedBinaryPayload = new String(subscriptionResponseMessage.getBinaryMessage().getMessage(),
                        StandardCharsets.UTF_8);
                receivedPayload = (JsonObject) Jsoner.deserialize(decodedBinaryPayload);
                topic = subscriptionResponseMessage.getBinaryMessage().getContext().getTopic();
                LOGGER.trace("Received binary message: {} on topic {}", decodedBinaryPayload, topic);
            }
            retVal = new Pair<>(topic, Message.build(receivedPayload));
        }
        catch (Exception e)
        {
            LOGGER.error("Problem decoding IPC payload into Message on topic {}: {}. Ignoring message",
                    topicFilter, e.toString());
        }
        return retVal;
    }
}
