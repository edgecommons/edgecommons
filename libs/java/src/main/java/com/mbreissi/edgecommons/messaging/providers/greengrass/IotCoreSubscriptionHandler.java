/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.messaging.providers.greengrass;


import com.mbreissi.edgecommons.messaging.Message;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import oshi.util.tuples.Pair;
import software.amazon.awssdk.aws.greengrass.model.IoTCoreMessage;
import software.amazon.awssdk.aws.greengrass.model.MQTTMessage;

import java.util.function.BiConsumer;

public class IotCoreSubscriptionHandler extends SubscriptionHandler<IoTCoreMessage>
{
    protected static final Logger LOGGER = LogManager.getLogger(IotCoreSubscriptionHandler.class);

    public IotCoreSubscriptionHandler(String topicFilter, BiConsumer<String, Message> callback, int maxConcurrency, int maxMessages)
    {
        super(topicFilter, callback, maxConcurrency, maxMessages);
    }

    @Override
    Pair<String, Message> parseRawPayload(IoTCoreMessage iotCoreMessage)
    {
        LOGGER.debug("IoT Core message received on subscription to topic filter '{}'", topicFilter);
        Pair<String, Message> retVal = null;
        try
        {
            String topic = iotCoreMessage.getMessage().getTopicName();
            MQTTMessage mqttMessage = iotCoreMessage.getMessage();
            Message msg = Message.fromBytes(mqttMessage.getPayload());
            retVal = new Pair<>(topic, msg);
        } catch (IllegalArgumentException e) {
            LOGGER.warn("Problem decoding IoT Core payload into EdgeCommons protobuf Message on topic {}: {}.  Ignoring message.",
                    topicFilter, e.toString());
        } catch (Exception e) {
            LOGGER.error("Unexpected error while parsing IoT Core payload: {}. Ignoring message", e.toString());
        }
        return retVal;
    }
}
