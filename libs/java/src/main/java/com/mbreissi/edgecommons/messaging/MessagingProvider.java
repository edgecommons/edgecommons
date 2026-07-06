/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.messaging;

import com.google.gson.JsonObject;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import software.amazon.awssdk.aws.greengrass.model.QOS;

import java.time.Duration;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.TimeoutException;
import java.util.function.BiConsumer;

// NOTE: not sealed — its impls live in sub-packages and sealed cross-package
// permits requires a named module (this lib is built as an unnamed-module JAR).
public abstract class MessagingProvider
{
    protected static final Logger LOGGER = LogManager.getLogger(MessagingProvider.class);

    /**
     * The built-in {@code request()} deadline (seconds) that applies until the config-model default
     * ({@code messaging.requestTimeoutSeconds}) is late-bound after {@code ConfigManager}
     * construction (UNS-CANONICAL-DESIGN §5 / D-U5). Deliberately non-zero so the CONFIG_COMPONENT
     * bootstrap request gets a deadline instead of hanging forever.
     */
    public static final int DEFAULT_REQUEST_TIMEOUT_SECONDS = 30;

    /**
     * The default {@code request()} deadline applied when a call supplies no per-call timeout.
     * {@code null}/zero/negative = disabled. Starts as the built-in
     * {@link #DEFAULT_REQUEST_TIMEOUT_SECONDS}; re-bound from
     * {@code messaging.requestTimeoutSeconds} once the config manager exists.
     */
    private volatile Duration defaultRequestTimeout = Duration.ofSeconds(DEFAULT_REQUEST_TIMEOUT_SECONDS);

    /**
     * The single shared lazy deadline timer thread for this provider (one 1-thread daemon
     * {@link ScheduledExecutorService} per provider, UNS-CANONICAL-DESIGN §5.2). Created on the
     * first request that arms a deadline; shut down by {@link #close()}.
     */
    private volatile ScheduledExecutorService requestDeadlineScheduler;
    private final Object requestDeadlineLock = new Object();

    /**
     * Sets the default {@code request()} deadline (the late-bind hook for
     * {@code messaging.requestTimeoutSeconds}, §5/D-U5). {@code null} or a zero/negative duration
     * disables the default deadline; an explicit per-call timeout always wins over this default.
     *
     * @param timeout the new default deadline, or {@code null}/{@link Duration#ZERO} to disable
     */
    public void setDefaultRequestTimeout(Duration timeout)
    {
        this.defaultRequestTimeout = timeout;
    }

    /**
     * The current default {@code request()} deadline (may be {@code null}/zero = disabled).
     *
     * @return the default deadline currently in effect
     */
    public Duration getDefaultRequestTimeout()
    {
        return defaultRequestTimeout;
    }

    /**
     * Resolves the deadline for one {@code request()} call: an explicit per-call timeout wins
     * (including {@link Duration#ZERO} = disabled for that call); {@code null} falls back to the
     * provider default. A zero/negative resolution yields {@code null} (no deadline).
     *
     * @param perCall the caller-supplied timeout, or {@code null} for the default
     * @return the effective deadline, or {@code null} when disabled
     */
    protected Duration effectiveRequestTimeout(Duration perCall)
    {
        Duration chosen = perCall != null ? perCall : defaultRequestTimeout;
        if (chosen == null || chosen.isZero() || chosen.isNegative())
        {
            return null;
        }
        return chosen;
    }

    /**
     * Arms the framework-owned deadline timer for a request at send time (§5). When the deadline
     * fires and wins the request's settle CAS ({@link ReplyFuture#trySettle()}), it (1) runs the
     * provider-supplied cleanup (unsubscribe the ephemeral reply topic, remove the pending entry)
     * and (2) completes the future exceptionally with a {@link TimeoutException} — even if the
     * caller never awaits the future (the reply-subscription leak fix). A no-op when
     * {@code timeout} is {@code null} (deadline disabled).
     *
     * @param future  the request's reply future
     * @param timeout the effective deadline from {@link #effectiveRequestTimeout}, or {@code null}
     * @param cleanup unsubscribes the reply topic and removes the pending entry (provider-specific)
     */
    protected void armRequestDeadline(ReplyFuture future, Duration timeout, Runnable cleanup)
    {
        if (timeout == null)
        {
            return;
        }
        final ScheduledFuture<?> task;
        try
        {
            task = scheduleRequestDeadline(future, timeout, cleanup);
        }
        catch (java.util.concurrent.RejectedExecutionException e)
        {
            // The provider is closing (scheduler shut down): no deadline can be armed. The request
            // proceeds deadline-less, matching the pre-§5 behavior on the shutdown path.
            LOGGER.warn("Provider is closing; request on reply topic '{}' proceeds without a deadline",
                    future.replyTopic);
            return;
        }
        future.setDeadlineTask(task);
    }

    /** Schedules the §5 deadline task (see {@link #armRequestDeadline}). */
    private ScheduledFuture<?> scheduleRequestDeadline(ReplyFuture future, Duration timeout, Runnable cleanup)
    {
        return requestDeadlineScheduler().schedule(() -> {
            if (!future.trySettle())
            {
                return;  // reply or cancel won the settle race — the deadline no-ops
            }
            try
            {
                cleanup.run();
            }
            catch (Exception e)
            {
                LOGGER.warn("Request-deadline cleanup for reply topic '{}' failed: {}",
                        future.replyTopic, e.toString());
            }
            future.completeExceptionally(new TimeoutException(
                    "request timed out after " + timeout.toMillis()
                    + " ms waiting for a reply on '" + future.replyTopic + "'"));
        }, timeout.toMillis(), TimeUnit.MILLISECONDS);
    }

    /** Lazily creates the shared single-thread daemon deadline scheduler for this provider. */
    private ScheduledExecutorService requestDeadlineScheduler()
    {
        ScheduledExecutorService scheduler = requestDeadlineScheduler;
        if (scheduler == null)
        {
            synchronized (requestDeadlineLock)
            {
                scheduler = requestDeadlineScheduler;
                if (scheduler == null)
                {
                    scheduler = Executors.newSingleThreadScheduledExecutor(r -> {
                        Thread t = new Thread(r, "edgecommons-request-deadline");
                        t.setDaemon(true);
                        return t;
                    });
                    requestDeadlineScheduler = scheduler;
                }
            }
        }
        return scheduler;
    }

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

    /**
     * {@code request()} with a per-call deadline (UNS-CANONICAL-DESIGN §5): {@code null} uses the
     * configured default ({@code messaging.requestTimeoutSeconds}); {@link Duration#ZERO} disables
     * the deadline for this call; an explicit value always wins over the default.
     */
    public abstract ReplyFuture request(String topic, Message message, Duration timeout);
    public abstract void cancelRequest(ReplyFuture future);
    public abstract void reply(Message request, Message reply);

    public abstract ReplyFuture requestFromIoTCore(String topic, Message request);

    /** IoT Core variant of {@link #request(String, Message, Duration)}. */
    public abstract ReplyFuture requestFromIoTCore(String topic, Message request, Duration timeout);
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

    /**
     * Releases any resources held by this provider. The base implementation shuts down the shared
     * request-deadline scheduler (if one was ever created); subclasses overriding {@code close()}
     * must call {@code super.close()}.
     */
    public void close()
    {
        ScheduledExecutorService scheduler = requestDeadlineScheduler;
        if (scheduler != null)
        {
            scheduler.shutdownNow();
        }
    }
}
