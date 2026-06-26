/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.messaging;

import com.google.gson.JsonObject;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import software.amazon.awssdk.aws.greengrass.model.QOS;

import java.util.function.BiConsumer;

// NOTE: not sealed — its impls live in sub-packages and sealed cross-package
// permits requires a named module (this lib is built as an unnamed-module JAR).
public abstract class MessagingProvider
{
    protected static final Logger LOGGER = LogManager.getLogger(MessagingProvider.class);

    public abstract void publish(String topic, Message message);
    public abstract void publishToIoTCore(String topic, Message message, QOS qos);

    public abstract void publishRaw(String topic, JsonObject payload);
    public abstract void publishToIoTCoreRaw(String topic, JsonObject payload, QOS qos);

    public abstract void subscribe(String topicFilter, BiConsumer<String, Message> callback,
                                   int maxConcurrency, int maxMessages);
    public abstract void subscribeToIoTCore(String topicFilter, BiConsumer<String, Message> callback, QOS qos,
                                            int maxConcurrency, int maxMessages);

    /** Backward-compatible overload: uses the default per-subscription queue bound. */
    public void subscribe(String topicFilter, BiConsumer<String, Message> callback, int maxConcurrency)
    {
        subscribe(topicFilter, callback, maxConcurrency, MessagingClient.DEFAULT_MAX_MESSAGES);
    }

    /** Backward-compatible overload: uses the default per-subscription queue bound. */
    public void subscribeToIoTCore(String topicFilter, BiConsumer<String, Message> callback, QOS qos, int maxConcurrency)
    {
        subscribeToIoTCore(topicFilter, callback, qos, maxConcurrency, MessagingClient.DEFAULT_MAX_MESSAGES);
    }
    public abstract void unsubscribe(String topicFilter);

    public abstract void unsubscribeFromIoTCore(String topicFilter);

    public abstract ReplyFuture request(String topic, Message message);
    public abstract void cancelRequest(ReplyFuture future);
    public abstract void reply(Message request, Message reply);

    public abstract ReplyFuture requestFromIoTCore(String topic, Message request);
    public abstract void cancelRequestFromIoTCore(ReplyFuture future);
    public abstract void replyToIoTCore(Message request, Message reply);

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
                return topicFilter.length() - filterPos > 1
                        && topicFilter.startsWith("/#", filterPos);
            }
        }
        return false;
    }

    public abstract Object getNativeLocalClient();

    public abstract Object getNativeIotCoreClient();

    /**
     * Whether the underlying transport is currently connected — the messaging input to the readiness
     * model (FR-HB-2): {@code /readyz} requires {@code connected() && ready && !shuttingDown}. For the
     * dual-MQTT provider this reflects the <em>local</em> broker link (the edge-critical half); for
     * the Greengrass IPC provider it is {@code true} once the IPC client is built. The default is
     * {@code false} (a provider that does not report connectivity is treated as not-ready).
     *
     * @return {@code true} if the transport is connected
     */
    public boolean connected() {
        return false;
    }

    /** Releases any resources held by this provider (connections, threads). Default no-op. */
    public void close() {}
}
