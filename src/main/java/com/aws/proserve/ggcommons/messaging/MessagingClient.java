package com.aws.proserve.ggcommons.messaging;

import com.aws.proserve.ggcommons.messaging.providers.GreengrassIpcProvider;
import com.aws.proserve.ggcommons.messaging.providers.MqttProvider;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.UUID;
import java.util.function.BiConsumer;

public class MessagingClient
{
    protected static final Logger LOGGER = LogManager.getLogger(MessagingClient.class);

    static MessagingProvider messagingProvider = null;

    public static void init(String[] messagingArgs, boolean receiveOwnMessages)
    {
        switch (messagingArgs[0].toUpperCase())
        {
            case "IPC":
                LOGGER.info("IPC specified in command line.  Using Greengrass IPC.");
                messagingProvider = new GreengrassIpcProvider(messagingArgs, receiveOwnMessages);
                break;
            case "MQTT":
                LOGGER.info("MQTT specified in command line.  Using MqttClient");
                messagingProvider = new MqttProvider(messagingArgs, UUID.randomUUID().toString());
                break;
            default:
                LOGGER.fatal("Invalid messaging provider specified in command line: must be either 'MQTT' or 'IPC'");
                System.exit(1);
        }
    }

    public static void publish(String topic, Message msg)
    {
        messagingProvider.publish(topic, msg);
        LOGGER.debug("Published IPC message on topic '{}': {}", topic, msg.toString());
    }

    public static void subscribe(String topicFilter, BiConsumer<String, Message> callback)
    {
        messagingProvider.subscribe(topicFilter, callback);
        LOGGER.debug("Subscribed to IPC messages on topic filter {}", topicFilter);
    }

    public static void reply(Message request, Message reply)
    {
        MessageHeader header = request.getHeader();
        if (header != null && header.getReplyTo() != null) {
            messagingProvider.publish(request.getHeader().getReplyTo(), reply);
            LOGGER.debug("Published reply on topic '{}: {}", request.getHeader().getReplyTo(), reply.toString());
        } else {
            LOGGER.warn("Missing header and/or 'reply_to' field in request message.  Unable to publish reply: {}", reply.toString());
        }
    }

    public static void unsubscribe(String topicFilter)
    {
        messagingProvider.unsubscribe(topicFilter);
        LOGGER.debug("Unsubscribed to IPC messages on topic filter {}", topicFilter);
    }

    public static boolean topicMatchesFilter(String topicFilter, String topic)
    {
        return MessagingProvider.topicMatchesFilter(topicFilter, topic);
    }
}
