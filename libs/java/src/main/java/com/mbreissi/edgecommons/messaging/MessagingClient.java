/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.messaging;

import com.mbreissi.edgecommons.ParsedCommandLine;
import com.mbreissi.edgecommons.messaging.providers.standalone.StandaloneMessagingProvider;
import com.mbreissi.edgecommons.messaging.providers.greengrass.GreengrassMessagingProvider;
import com.mbreissi.edgecommons.uns.Uns;
import com.mbreissi.edgecommons.uns.UnsClass;
import com.google.gson.JsonObject;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.time.Duration;
import java.util.function.BiConsumer;

/**
 * A client for handling messaging operations in Greengrass components. This class provides methods for
 * publishing and subscribing to messages, making requests, and handling replies in both local and IoT Core
 * communication contexts.
 */
public class MessagingClient
{
    protected static final Logger LOGGER = LogManager.getLogger(MessagingClient.class);

    /** Default per-subscription queue bound when a caller does not specify one (drop-oldest on overflow). */
    public static final int DEFAULT_MAX_MESSAGES = 10_000;

    private MessagingProvider messagingProvider;

    /**
     * Whether the reserved-class publish guard also checks the class token at topic position 5 —
     * this component's {@code topic.includeRoot} setting (UNS-CANONICAL-DESIGN §4.1, D-U24).
     * Late-bound from the {@code ConfigManager} via {@link #setGuardIncludeRoot(boolean)} right
     * after config loads (the messaging client is constructed BEFORE config because the IPC-backed
     * config sources need it); {@code false} pre-bind — nothing publishes rooted topics pre-config.
     */
    private volatile boolean guardIncludeRoot = false;

    /** Lazily-created privileged internal-publish seam ({@link #reservedPublisher()}, §4.2). */
    private volatile ReservedPublisher reservedPublisher;

    /**
     * Protected no-arg constructor for testing/subclassing (e.g. mock messaging clients).
     * Leaves the underlying provider null; subclasses are expected to override the messaging methods.
     */
    protected MessagingClient() {
    }

    /**
     * Package-private constructor for builder pattern. Branches on the resolved
     * {@link com.mbreissi.edgecommons.platform.Transport} (DESIGN-core §4.2 transport-injection
     * site), not on a legacy mode enum.
     */
    MessagingClient(ParsedCommandLine cmdLine, boolean receiveOwnMessages) {
        switch (cmdLine.transport) {
            case IPC:
                LOGGER.info("IPC transport selected. Using Greengrass IPC.");
                this.messagingProvider = new GreengrassMessagingProvider(receiveOwnMessages);
                break;
            case MQTT:
                LOGGER.info("MQTT transport selected. Using dual MQTT clients.");
                try {
                    MessagingConfiguration config = MessagingConfiguration.loadFromFile(cmdLine.standaloneConfigPath);
                    this.messagingProvider = new StandaloneMessagingProvider(config, cmdLine.thingName);
                } catch (Exception e) {
                    LOGGER.fatal("Failed to load standalone messaging configuration: {}", e.getMessage());
                    throw new RuntimeException("Failed to load standalone messaging configuration: " + e.getMessage(), e);
                }
                break;
            default:
                LOGGER.fatal("Invalid transport specified: {}", cmdLine.transport);
                throw new RuntimeException("Invalid transport specified: " + cmdLine.transport);
        }
    }

    /**
     * Publishes a message to a specified topic. Client-chosen topics targeting a reserved UNS
     * class ({@code state | metric | cfg | log}) are rejected (§4.1) — the library publishers own
     * those classes.
     *
     * @param topic The topic to publish the message to
     * @param msg The message to publish
     * @throws ReservedTopicException when the topic targets a reserved UNS class
     */
    public void publish(String topic, Message msg)
    {
        checkReservedTopic(topic);
        messagingProvider.publish(topic, msg);
        LOGGER.debug("Published IPC message on topic '{}': {}", topic, msg.toString());
    }

    /**
     * Publishes a message to the northbound transport with specified quality of service. Reserved-class UNS
     * topics are rejected (§4.1).
     *
     * @param topic The northbound topic to publish to
     * @param msg The message to publish
     * @param qos The quality of service level for message delivery
     * @throws ReservedTopicException when the topic targets a reserved UNS class
     */
    public void publishNorthbound(String topic, Message msg, Qos qos)
    {
        checkReservedTopic(topic);
        messagingProvider.publishNorthbound(topic, msg, qos);
        LOGGER.debug("Published IoT Core message on topic '{}': {}", topic, msg.toString());
    }

    /**
     * Strict local publish of a message envelope. Completion means the provider observed the
     * transport acknowledgement required for QoS 1; timeout, disconnect, and unsupported
     * transports throw and are never reported as success.
     */
    public void publishConfirmed(String topic, Message msg, Qos qos, Duration timeout)
    {
        if (msg == null)
        {
            throw new NullPointerException("msg must not be null");
        }
        publishConfirmed(topic, msg.toBytes(), qos, timeout);
    }

    /**
     * Strict local publish of exact pre-encoded envelope bytes. This is the durable-outbox path:
     * retries can reuse the identical bytes and envelope UUID.
     */
    public void publishConfirmed(String topic, byte[] encodedMessage, Qos qos, Duration timeout)
    {
        checkReservedTopic(topic);
        validateConfirmedEnvelope(encodedMessage);
        confirmedProvider().publishConfirmed(topic, encodedMessage.clone(), qos, timeout);
        LOGGER.debug("Confirmed local publish on topic '{}'", topic);
    }

    /** Strict northbound publish of a message envelope at explicit QoS 1. */
    public void publishNorthboundConfirmed(String topic, Message msg, Qos qos, Duration timeout)
    {
        if (msg == null)
        {
            throw new NullPointerException("msg must not be null");
        }
        publishNorthboundConfirmed(topic, msg.toBytes(), qos, timeout);
    }

    /** Strict northbound publish of exact pre-encoded envelope bytes at explicit QoS 1. */
    public void publishNorthboundConfirmed(String topic, byte[] encodedMessage, Qos qos,
                                            Duration timeout)
    {
        checkReservedTopic(topic);
        validateConfirmedEnvelope(encodedMessage);
        confirmedProvider().publishNorthboundConfirmed(
                topic, encodedMessage.clone(), qos, timeout);
        LOGGER.debug("Confirmed northbound publish on topic '{}'", topic);
    }

    private MessagingProvider confirmedProvider()
    {
        if (messagingProvider == null)
        {
            throw new UnsupportedOperationException(
                    getClass().getName() + " has no confirmed-publish provider");
        }
        return messagingProvider;
    }

    private static void validateConfirmedEnvelope(byte[] encodedMessage)
    {
        if (encodedMessage == null)
        {
            throw new NullPointerException("encodedMessage must not be null");
        }
        try
        {
            // Parse and pass through the canonical encoder's structural checks without replacing
            // the caller's bytes. Providers still receive the exact durable-outbox representation.
            Message.fromBytes(encodedMessage).toBytes();
        }
        catch (RuntimeException e)
        {
            throw new IllegalArgumentException(
                    "confirmed publish requires a valid EdgeCommons envelope", e);
        }
    }

    /**
     * Publishes a raw JSON object to a topic without wrapping it in a Message. Reserved-class UNS
     * topics are rejected (§4.1, D-U8).
     *
     * @param topic The topic to publish to
     * @param metricObject The JSON object to publish
     * @throws ReservedTopicException when the topic targets a reserved UNS class
     */
    public void publishRaw(String topic, JsonObject metricObject)
    {
        checkReservedTopic(topic);
        messagingProvider.publishRaw(topic, metricObject);
    }

    /**
     * Publishes a raw JSON object to a topic without wrapping it in a Message. Reserved-class UNS
     * topics are rejected (§4.1, D-U8).
     *
     * @param topic The topic to publish to
     * @param metricObject The JSON object to publish
     * @throws ReservedTopicException when the topic targets a reserved UNS class
     */
    public void publishNorthboundRaw(String topic, JsonObject metricObject, Qos qos)
    {
        checkReservedTopic(topic);
        messagingProvider.publishNorthboundRaw(topic, metricObject, qos);
    }

    /**
     * Subscribes to messages on a topic with a callback for message handling.
     *
     * @param topicFilter The topic filter to subscribe to
     * @param callback The callback to invoke when messages are received
     */
    public void subscribe(String topicFilter, BiConsumer<String, Message> callback)
    {
        subscribe(topicFilter, callback, -1);
    }

    public void subscribe(String topicFilter, BiConsumer<String, Message> callback, int maxConcurrency)
    {
        subscribe(topicFilter, callback, maxConcurrency, DEFAULT_MAX_MESSAGES);
    }

    /**
     * @param maxMessages per-subscription queue bound; when the buffer is full the oldest pending
     *     message is dropped with a warning (parity with the Rust/TS providers). {@code <= 0} =
     *     unbounded. Omitting it uses {@link #DEFAULT_MAX_MESSAGES}.
     */
    public void subscribe(String topicFilter, BiConsumer<String, Message> callback, int maxConcurrency, int maxMessages)
    {
        messagingProvider.subscribe(topicFilter, callback, maxConcurrency, maxMessages);
        LOGGER.debug("Subscribed to IPC messages on topic filter {}", topicFilter);
    }

    /**
     * Bounded local subscription whose successful return proves MQTT SUBACK or Greengrass
     * subscription-operation completion. There is deliberately no fallback to {@link #subscribe}.
     */
    public void subscribeAcknowledged(String topicFilter,
                                      BiConsumer<String, Message> callback,
                                      int maxConcurrency,
                                      int maxMessages,
                                      Duration timeout)
    {
        if (messagingProvider == null)
        {
            throw new UnsupportedOperationException(
                    getClass().getName() + " has no acknowledged-subscription provider");
        }
        messagingProvider.subscribeAcknowledged(
                topicFilter, callback, maxConcurrency, maxMessages, timeout);
        LOGGER.debug("Acknowledged local subscription on topic filter {}", topicFilter);
    }

    /**
     * Subscribes to messages from IoT Core with specified quality of service.
     *
     * @param topicFilter The topic filter to subscribe to
     * @param callback The callback to invoke when messages are received
     * @param qos The quality of service level for the subscription
     */
    public void subscribeNorthbound(String topicFilter, BiConsumer<String, Message> callback, Qos qos)
    {
        subscribeNorthbound(topicFilter, callback, qos, -1);
    }

    public void subscribeNorthbound(String topicFilter, BiConsumer<String, Message> callback, Qos qos, int maxConcurrency)
    {
        subscribeNorthbound(topicFilter, callback, qos, maxConcurrency, DEFAULT_MAX_MESSAGES);
    }

    /** @param maxMessages per-subscription queue bound (drop-oldest + warn on overflow); {@code <= 0} = unbounded. */
    public void subscribeNorthbound(String topicFilter, BiConsumer<String, Message> callback, Qos qos, int maxConcurrency, int maxMessages)
    {
        messagingProvider.subscribeNorthbound(topicFilter, callback, qos, maxConcurrency, maxMessages);
        LOGGER.debug("Subscribed to northbound messages on topic filter {}", topicFilter);
    }

    /**
     * Sends a request message and returns a future for handling the reply. The reply future
     * carries the framework-owned default deadline ({@code messaging.requestTimeoutSeconds},
     * default 30 s, UNS-CANONICAL-DESIGN §5): on expiry the ephemeral reply subscription is
     * cleaned up and the future completes exceptionally with a
     * {@link java.util.concurrent.TimeoutException} — even if the caller never awaits it.
     *
     * @param topic The topic to send the request to
     * @param request The request message
     * @return A ReplyFuture for handling the response
     */
    public ReplyFuture request(String topic, Message request)
    {
        checkReservedTopic(topic);
        return messagingProvider.request(topic, request);
    }

    /**
     * {@link #request(String, Message)} with an explicit per-call deadline (§5, D-U5): an explicit
     * value always wins over the configured default; {@code null} uses the default;
     * {@link Duration#ZERO} disables the deadline for this call.
     *
     * @param topic The topic to send the request to
     * @param request The request message
     * @param timeout The per-call deadline ({@code null} = default, zero = disabled)
     * @return A ReplyFuture for handling the response
     */
    public ReplyFuture request(String topic, Message request, Duration timeout)
    {
        checkReservedTopic(topic);
        return messagingProvider.request(topic, request, timeout);
    }

    /**
     * Sends a request message to IoT Core and returns a future for handling the reply. Carries the
     * same framework-owned default deadline as {@link #request(String, Message)}.
     *
     * @param topic The northbound topic to send the request to
     * @param request The request message
     * @return A ReplyFuture for handling the response
     */
    public ReplyFuture requestNorthbound(String topic, Message request)
    {
        checkReservedTopic(topic);
        return messagingProvider.requestNorthbound(topic, request);
    }

    /**
     * {@link #requestNorthbound(String, Message)} with an explicit per-call deadline (§5, D-U5):
     * an explicit value wins over the configured default; {@code null} uses the default;
     * {@link Duration#ZERO} disables the deadline for this call.
     *
     * @param topic The northbound topic to send the request to
     * @param request The request message
     * @param timeout The per-call deadline ({@code null} = default, zero = disabled)
     * @return A ReplyFuture for handling the response
     */
    public ReplyFuture requestNorthbound(String topic, Message request, Duration timeout)
    {
        checkReservedTopic(topic);
        return messagingProvider.requestNorthbound(topic, request, timeout);
    }

    /**
     * Late-binds the default {@code request()} deadline from the config model
     * ({@code messaging.requestTimeoutSeconds}, §5/D-U5). Called by the runtime right after the
     * {@code ConfigManager} exists (the messaging client is constructed first because the
     * IPC-backed config sources need it); until then the built-in 30 s applies — deliberately, so
     * the CONFIG_COMPONENT bootstrap request gets a deadline instead of hanging. {@code null} or
     * {@link Duration#ZERO} disables the default deadline. Safe no-op when no provider is wired
     * (test/subclass constructor).
     *
     * @param timeout the new default deadline ({@code null}/zero = disabled)
     */
    public void setDefaultRequestTimeout(Duration timeout)
    {
        if (messagingProvider != null)
        {
            messagingProvider.setDefaultRequestTimeout(timeout);
            LOGGER.debug("Default request timeout bound to {}", timeout);
        }
    }

    /**
     * The default {@code request()} deadline currently in effect on the underlying provider, or
     * {@code null} when no provider is wired.
     *
     * @return the default deadline (zero/{@code null} = disabled)
     */
    public Duration getDefaultRequestTimeout()
    {
        return messagingProvider == null ? null : messagingProvider.getDefaultRequestTimeout();
    }

    /**
     * Cancels a pending request and cleans up associated resources.
     *
     * @param replyFuture The ReplyFuture associated with the request to cancel
     */
    public void cancelRequest(ReplyFuture replyFuture)
    {
        messagingProvider.cancelRequest(replyFuture);
    }

    public void cancelRequestNorthbound(ReplyFuture replyFuture)
    {
        messagingProvider.cancelRequestNorthbound(replyFuture);
    }

    /**
     * Sends a reply to a received request message. The request's {@code reply_to} topic is
     * guarded like a client-chosen topic (§4.1, D-U8): a hostile requester could otherwise set
     * {@code header.reply_to} to a victim's reserved topic and turn an innocent responder into a
     * forger.
     *
     * @param request The original request message
     * @param reply The reply message
     * @throws ReservedTopicException when the request's reply topic targets a reserved UNS class
     */
    public void reply(Message request, Message reply)
    {
        checkReservedTopic(replyTopicOf(request));
        messagingProvider.reply(request, reply);
        LOGGER.debug("Published reply on topic '{}: {}", request.getHeader().getReplyTo(), reply.toString());
    }

    /**
     * Sends a guarded local reply and waits for positive QoS-1 transport confirmation. The reply
     * inherits the request correlation before it is encoded.
     */
    public void replyConfirmed(Message request, Message reply, Duration timeout)
    {
        validateReplyTarget(request);
        if (reply == null)
        {
            throw new NullPointerException("reply must not be null");
        }
        reply.setCorrelationId(request.getHeader().getCorrelationId());
        publishConfirmed(request.getHeader().getReplyTo(), reply, Qos.AT_LEAST_ONCE, timeout);
    }

    /** Guarded northbound counterpart of {@link #replyConfirmed(Message, Message, Duration)}. */
    public void replyNorthboundConfirmed(Message request, Message reply, Duration timeout)
    {
        validateReplyTarget(request);
        if (reply == null)
        {
            throw new NullPointerException("reply must not be null");
        }
        reply.setCorrelationId(request.getHeader().getCorrelationId());
        publishNorthboundConfirmed(
                request.getHeader().getReplyTo(), reply, Qos.AT_LEAST_ONCE, timeout);
    }

    /**
     * IoT Core variant of {@link #reply(Message, Message)} — the request's {@code reply_to} topic
     * is guarded the same way.
     *
     * @throws ReservedTopicException when the request's reply topic targets a reserved UNS class
     */
    public void replyNorthbound(Message request, Message reply)
    {
        checkReservedTopic(replyTopicOf(request));
        messagingProvider.replyNorthbound(request, reply);
    }

    /** The request's {@code reply_to} topic, or {@code null} when it has no header/reply-to. */
    private static String replyTopicOf(Message request)
    {
        return request == null || request.getHeader() == null ? null : request.getHeader().getReplyTo();
    }

    /**
     * Validates a received request's reply target through the same reserved-topic guard used by
     * {@link #reply(Message, Message)}. Deferred-reply registries call this before provisioning a
     * token, so a hostile target never becomes retained reply state.
     *
     * @throws IllegalArgumentException when the request has no non-empty {@code reply_to}
     * @throws ReservedTopicException when the target is a library-owned UNS class
     */
    public void validateReplyTarget(Message request)
    {
        String replyTo = replyTopicOf(request);
        if (replyTo == null || replyTo.isEmpty())
        {
            throw new IllegalArgumentException("request requires a non-empty reply_to");
        }
        checkReservedTopic(replyTo);
    }

    /**
     * Late-binds the reserved-class guard's {@code topic.includeRoot} flag from the config model
     * (§4.1, D-U24). Called by the runtime right after the {@code ConfigManager} exists; before
     * the bind only the always-checked class position 4 applies.
     *
     * @param includeRoot this component's resolved {@code topic.includeRoot} setting
     */
    public void setGuardIncludeRoot(boolean includeRoot)
    {
        this.guardIncludeRoot = includeRoot;
        LOGGER.debug("Reserved-topic guard includeRoot bound to {}", includeRoot);
    }

    /**
     * The reserved-class publish guard (UNS-CANONICAL-DESIGN §4.1): rejects a client-chosen topic
     * whose class position holds a reserved token ({@code state | metric | cfg | log}). The class
     * position is topic level 4 (0-based) always — the rootless grammar
     * {@code ecv1/{device}/{component}/{instance}/{class}} — and level 5 <b>only when this
     * component's {@code topic.includeRoot} is true</b> (checking it unconditionally would
     * false-positive on legitimate app channels like {@code ecv1/d/c/i/app/state}). Non-
     * {@code ecv1} topics pass untouched ({@code edgecommons/reply-…}, {@code cloudwatch/metric/put},
     * foreign MQTT bridging). {@code subscribe*} is never guarded (consumers must read reserved
     * classes).
     *
     * @param topic the client-chosen topic ({@code null} passes — provider-level validation owns it)
     * @throws ReservedTopicException when the topic targets a reserved UNS class
     */
    private void checkReservedTopic(String topic)
    {
        UnsClass reserved = reservedClassOf(topic, guardIncludeRoot);
        if (reserved != null)
        {
            throw new ReservedTopicException(topic, reserved.token);
        }
    }

    /**
     * The §4.1 guard predicate: the reserved class the topic targets, or {@code null} when the
     * topic is allowed. Static and package-visible for the guard's unit tests.
     *
     * @param topic       the topic to test
     * @param includeRoot whether the position-5 check applies (this component's
     *                    {@code topic.includeRoot})
     * @return the reserved {@link UnsClass}, or {@code null} when the topic passes
     */
    static UnsClass reservedClassOf(String topic, boolean includeRoot)
    {
        if (topic == null || !topic.startsWith(Uns.ROOT))
        {
            return null;
        }
        String[] tokens = topic.split("/", -1);
        if (!Uns.ROOT.equals(tokens[0]))
        {
            return null;
        }
        // D‑U28: the instance slot is optional. {component} sits at index 2 rootless / 3 rooted, so
        // the class is the token right after it (component scope) or one further right (an instance
        // token is present). An instance can never be a class token, so the class is the token after
        // {component} iff that token is a class token, else the one after it.
        int base = includeRoot ? 4 : 3;
        if (tokens.length > base)
        {
            int classIdx = UnsClass.fromToken(tokens[base]) != null ? base : base + 1;
            if (classIdx < tokens.length)
            {
                UnsClass cls = UnsClass.fromToken(tokens[classIdx]);
                if (cls != null && UnsClass.RESERVED.contains(cls))
                {
                    return cls;
                }
            }
        }
        return null;
    }

    /**
     * Returns the privileged internal-publish seam (UNS-CANONICAL-DESIGN §4.2, D-U4): a
     * {@link ReservedPublisher} whose publishes BYPASS the reserved-class guard.
     *
     * <p><b>Library-internal.</b> Public only because the library's own publishers (heartbeat
     * state keepalive, the {@code Messaging} metric target, the effective-config publisher) live
     * in other packages. Component code should not call this — the guard it bypasses is there to
     * keep the library-owned UNS classes consistent (broker ACLs are the security boundary).
     *
     * @return the reserved publisher bound to this client
     */
    public ReservedPublisher reservedPublisher()
    {
        ReservedPublisher publisher = reservedPublisher;
        if (publisher == null)
        {
            publisher = new ReservedPublisher(this);
            reservedPublisher = publisher;
        }
        return publisher;
    }

    /**
     * Unguarded local/IPC publish — the {@link ReservedPublisher} delegate (§4.2). Protected so
     * mock messaging clients can record reserved publishes like regular ones.
     */
    protected void publishReserved(String topic, Message msg)
    {
        messagingProvider.publish(topic, msg);
        LOGGER.debug("Published reserved message on topic '{}'", topic);
    }

    /** Unguarded raw local/IPC publish — the {@link ReservedPublisher} delegate (§4.2). */
    protected void publishReservedRaw(String topic, JsonObject payload)
    {
        messagingProvider.publishRaw(topic, payload);
    }

    /** Unguarded IoT Core publish — the {@link ReservedPublisher} delegate (§4.2). */
    protected void publishReservedNorthbound(String topic, Message msg, Qos qos)
    {
        messagingProvider.publishNorthbound(topic, msg, qos);
        LOGGER.debug("Published reserved IoT Core message on topic '{}'", topic);
    }

    /**
     * Unsubscribes from messages on a topic.
     *
     * @param topicFilter The topic filter to unsubscribe from
     */
    public void unsubscribe(String topicFilter)
    {
        messagingProvider.unsubscribe(topicFilter);
        LOGGER.debug("Unsubscribed to IPC messages on topic filter {}", topicFilter);
    }

    public void unsubscribeNorthbound(String topicFilter)
    {
        messagingProvider.unsubscribeNorthbound(topicFilter);
        LOGGER.debug("Unsubscribed to IPC messages on topic filter {}", topicFilter);
    }

    /**
     * Checks if a topic matches a topic filter pattern.
     *
     * @param topicFilter The topic filter pattern
     * @param topic The topic to check
     * @return true if the topic matches the filter, false otherwise
     */
    public static boolean topicMatchesFilter(String topicFilter, String topic)
    {
        return MessagingProvider.topicMatchesFilter(topicFilter, topic);
    }

    /**
     * Whether the messaging transport is currently connected — the messaging input to the health
     * readiness model (FR-HB-2). Delegates to the underlying provider
     * ({@link MessagingProvider#connected()}); returns {@code false} when no provider is wired (e.g.
     * the test/subclass no-arg constructor), so a runtime with no messaging is treated as not-ready.
     *
     * @return {@code true} if the transport is connected
     */
    public boolean connected()
    {
        return messagingProvider != null && messagingProvider.connected();
    }

    /**
     * Closes the underlying messaging provider, releasing connections and background threads.
     */
    public void close()
    {
        if (messagingProvider != null)
        {
            messagingProvider.close();
        }
    }

    /**
     * Returns the underlying native local messaging client implementation.
     *
     * @return The native messaging client object
     */
    public Object getNativeLocalClient()
    {
        return messagingProvider.getNativeLocalClient();
    }

    /**
     * Returns the underlying native iot core messaging client implementation.
     *
     * @return The native messaging client object
     */
    public Object getNativeNorthboundClient()
    {
        return messagingProvider.getNativeNorthboundClient();
    }

}
