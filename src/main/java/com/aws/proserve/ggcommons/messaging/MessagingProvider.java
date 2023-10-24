package com.aws.proserve.ggcommons.messaging;

import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import software.amazon.awssdk.aws.greengrass.model.QOS;

import java.util.function.BiConsumer;

public abstract class MessagingProvider
{
    protected static final Logger LOGGER = LogManager.getLogger(MessagingProvider.class);

    String[] messagingArgs;

    protected MessagingProvider(String[] messagingArgs)
    {
        this.messagingArgs = messagingArgs;
    }

    public abstract void publish(String topic, Message message);
    public abstract void publishToIoTCore(String topic, Message message, QOS qos);
    public abstract void subscribe(String topicFilter, BiConsumer<String, Message> callback,
                                   int maxConcurrency);
    public abstract void subscribeToIoTCore(String topicFilter, BiConsumer<String, Message> callback, QOS qos,
                                            int maxConcurrency);
    public abstract void unsubscribe(String topicFilter);

    public abstract void unsubscribeFromIoTCore(String topicFilter);

    public abstract ReplyFuture request(String topic, Message message);
    public abstract void cancelRequest(ReplyFuture future);
    public abstract void reply(Message request, Message reply);

    // Copied from open source Paho MQTT Java client
    // (https://github.com/eclipse/paho.mqtt.java/blob/master/org.eclipse.paho.client.mqttv3/src/main/java/org/eclipse/paho/client/mqttv3/MqttTopic.java)
    // Under the Eclipse Public License (https://github.com/eclipse/paho.mqtt.java/blob/master/LICENSE)
    /**
     * Check the supplied topic name and filter match
     *
     * @param topicFilter
     *            topic filter: wildcards allowed
     * @param topicName
     *            topic name: wildcards not allowed
     * @return true if the topic matches the filter
     * @throws IllegalArgumentException
     *             if the topic name or filter is invalid
     */
    public static boolean topicMatchesFilter(String topicFilter, String topicName) throws IllegalArgumentException
    {
        int topicPos = 0;
        int filterPos = 0;
        int topicLen = topicName.length();
        int filterLen = topicFilter.length();

//        MqttTopic.validate(topicFilter, true);
//        MqttTopic.validate(topicName, false);

        if (topicFilter.equals(topicName))
        {
            return true;
        }

        while (filterPos < filterLen && topicPos < topicLen)
        {
            if (topicFilter.charAt(filterPos) == '#')
            {
                /*
                 * next 'if' will break when topicFilter = topic/# and topicName topic/A/,
                 * but they are matched
                 */
                topicPos = topicLen;
                filterPos = filterLen;
                break;
            }
            if (topicName.charAt(topicPos) == '/' && topicFilter.charAt(filterPos) != '/')
                break;
            if (topicFilter.charAt(filterPos) != '+' && topicFilter.charAt(filterPos) != '#'
                    && topicFilter.charAt(filterPos) != topicName.charAt(topicPos))
                break;
            if (topicFilter.charAt(filterPos) == '+')
            { // skip until we meet the next separator, or end of string
                int nextpos = topicPos + 1;
                while (nextpos < topicLen && topicName.charAt(nextpos) != '/')
                    nextpos = ++topicPos + 1;
            }

            filterPos++;
            topicPos++;
        }

        if ((topicPos == topicLen) && (filterPos == filterLen))
        {
            return true;
        }
        else
        {
            /*
             * https://github.com/eclipse/paho.mqtt.java/issues/418
             * Covers edge case to match sport/# to sport
             */
            if ((topicFilter.length() - filterPos > 0) && (topicPos == topicLen))
            {
                if (topicName.charAt(topicPos - 1) == '/' && topicFilter.charAt(filterPos) == '#')
                    return true;
                if (topicFilter.length() - filterPos > 1
                        && topicFilter.substring(filterPos, filterPos + 2).equals("/#"))
                {
                    return true;
                }
            }
        }
        return false;
    }
}
