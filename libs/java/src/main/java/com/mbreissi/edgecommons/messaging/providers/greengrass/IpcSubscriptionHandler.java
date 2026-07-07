/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.messaging.providers.greengrass;


import com.mbreissi.edgecommons.messaging.Message;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import oshi.util.tuples.Pair;
import software.amazon.awssdk.aws.greengrass.model.SubscriptionResponseMessage;

import java.util.function.BiConsumer;

public class IpcSubscriptionHandler extends SubscriptionHandler<SubscriptionResponseMessage>
{
    protected static final Logger LOGGER = LogManager.getLogger(IpcSubscriptionHandler.class);

    public IpcSubscriptionHandler(String topicFilter, BiConsumer<String, Message> callback, int maxConcurrency, int maxMessages)
    {
        super(topicFilter, callback, maxConcurrency, maxMessages);
    }

    @Override
    Pair<String, Message> parseRawPayload(SubscriptionResponseMessage subscriptionResponseMessage)
    {
        LOGGER.debug("IPC Message received on subscription to topic filter '{}'", topicFilter);
        Pair<String, Message> retVal = null;
        try
        {
            String topic;
            if (subscriptionResponseMessage.getJsonMessage() != null)
            {
                topic = subscriptionResponseMessage.getJsonMessage().getContext().getTopic();
                LOGGER.warn("Received Greengrass JsonMessage on EdgeCommons subscription topic {}; ignoring non-protobuf payload",
                        topic);
                return null;
            }
            else
            {
                topic = subscriptionResponseMessage.getBinaryMessage().getContext().getTopic();
                LOGGER.trace("Received binary EdgeCommons message on topic {}", topic);
                retVal = new Pair<>(topic, Message.fromBytes(subscriptionResponseMessage.getBinaryMessage().getMessage()));
            }
        }
        catch (IllegalArgumentException e)
        {
            LOGGER.warn("Problem decoding IPC payload into EdgeCommons protobuf Message on topic {}: {}. Ignoring message",
                    topicFilter, e.toString());
        }
        catch (NullPointerException e)
        {
            LOGGER.error("Null pointer encountered while processing IPC payload on topic {}: {}. Ignoring message",
                    topicFilter, e.toString());
        }
        return retVal;
    }
}
