package com.aws.proserve.ggcommons.messaging;

import com.aws.proserve.ggcommons.messaging.providers.greengrass.GreengrassIpcProvider;
import com.aws.proserve.ggcommons.messaging.providers.MqttProvider;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import software.amazon.awssdk.aws.greengrass.model.QOS;

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

    public static void publishToIotCore(String topic, Message msg, QOS qos)
    {
        messagingProvider.publishToIoTCore(topic, msg,  qos);
        LOGGER.debug("Published IoT Core message on topic '{}': {}", topic, msg.toString());
    }

    public static void subscribe(String topicFilter, BiConsumer<String, Message> callback)
    {
        subscribe(topicFilter, callback, false);
    }
    public static void subscribe(String topicFilter, BiConsumer<String, Message> callback,
                                 boolean serializeProcessing)
    {
        messagingProvider.subscribe(topicFilter, callback, serializeProcessing);
        LOGGER.debug("Subscribed to IPC messages on topic filter {}", topicFilter);
    }

    public static void subscribeToIoTCore(String topicFilter, BiConsumer<String, Message> callback, QOS qos)
    {
        subscribeToIoTCore(topicFilter, callback, qos, false);
    }

    public static void subscribeToIoTCore(String topicFilter, BiConsumer<String, Message> callback, QOS qos,
                                          boolean serializeProcessing)
    {
        messagingProvider.subscribeToIoTCore(topicFilter, callback, qos, serializeProcessing);
        LOGGER.debug("Subscribed to IoT Core messages on topic filter {}", topicFilter);
    }

    public static ReplyFuture request(String topic, Message request)
    {
        return messagingProvider.request(topic, request);
    }

    public static void cancelRequest(ReplyFuture replyFuture)
    {
        messagingProvider.cancelRequest(replyFuture);
    }
    public static void reply(Message request, Message reply)
    {
        messagingProvider.reply(request, reply);
        LOGGER.debug("Published reply on topic '{}: {}", request.getHeader().getReplyTo(), reply.toString());
    }

    public static void unsubscribe(String topicFilter)
    {
        messagingProvider.unsubscribe(topicFilter);
        LOGGER.debug("Unsubscribed to IPC messages on topic filter {}", topicFilter);
    }

    public static void unsubscribeFromIoTCore(String topicFilter)
    {
        messagingProvider.unsubscribeFromIoTCore(topicFilter);
        LOGGER.debug("Unsubscribed to IPC messages on topic filter {}", topicFilter);
    }

    public static boolean topicMatchesFilter(String topicFilter, String topic)
    {
        return MessagingProvider.topicMatchesFilter(topicFilter, topic);
    }

}
