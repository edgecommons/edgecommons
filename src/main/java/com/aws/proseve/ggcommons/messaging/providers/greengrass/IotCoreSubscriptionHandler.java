package com.aws.proseve.ggcommons.messaging.providers.greengrass;


import com.github.cliftonlabs.json_simple.Jsoner;
import com.aws.proseve.ggcommons.messaging.Message;
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

    public IotCoreSubscriptionHandler(String topicFilter, BiConsumer<String, Message> callback, int maxConcurrency)
    {
        super(topicFilter, callback, maxConcurrency);
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
                msg = Message.build(Jsoner.deserialize(msgChars));
            }
            catch (Exception e)
            {
                msg = Message.build(msgChars);
            }
            retVal = new Pair<>(topic, msg);
        } catch (Exception e) {
            LOGGER.warn("Problem decoding IoT Core payload into Message on topic {}: {}.  Ignoring message.",
                    topicFilter, e.toString());
        }
        return retVal;
    }
}
