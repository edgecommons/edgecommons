/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.messaging.providers.greengrass;


import com.aws.proserve.ggcommons.messaging.Message;
import com.google.gson.Gson;
import com.google.gson.JsonObject;
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
                final Gson gson = new Gson();
                receivedPayload = gson.toJsonTree(subscriptionResponseMessage.getJsonMessage().getMessage()).getAsJsonObject();
                topic = subscriptionResponseMessage.getJsonMessage().getContext().getTopic();
                LOGGER.trace("Received json message: {} on topic {}", receivedPayload.toString(), topic);
            }
            else
            {
                String decodedBinaryPayload = new String(subscriptionResponseMessage.getBinaryMessage().getMessage(),
                        StandardCharsets.UTF_8);
                receivedPayload = new Gson().fromJson(decodedBinaryPayload, JsonObject.class);
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
