/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.messaging.providers.greengrass;


import com.aws.proserve.ggcommons.messaging.Message;
import com.aws.proserve.ggcommons.messaging.MessageBuilder;
import com.google.gson.Gson;
import com.google.gson.JsonObject;
import com.google.gson.JsonSyntaxException;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import oshi.util.tuples.Pair;
import software.amazon.awssdk.aws.greengrass.model.IoTCoreMessage;
import software.amazon.awssdk.aws.greengrass.model.MQTTMessage;

import java.nio.charset.StandardCharsets;
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
            String msgChars = new String(mqttMessage.getPayload(), StandardCharsets.UTF_8);
            Message msg;
            try
            {
                msg = MessageBuilder.fromObject(new Gson().fromJson(msgChars, JsonObject.class));
            }
            catch (JsonSyntaxException e)
            {
                msg = MessageBuilder.fromObject(msgChars);
            }
            retVal = new Pair<>(topic, msg);
        } catch (JsonSyntaxException | IllegalArgumentException e) {
            LOGGER.warn("Problem decoding IoT Core payload into Message on topic {}: {}.  Ignoring message.",
                    topicFilter, e.toString());
        } catch (Exception e) {
            LOGGER.error("Unexpected error while parsing IoT Core payload: {}. Ignoring message", e.toString());
        }
        return retVal;
    }
}
