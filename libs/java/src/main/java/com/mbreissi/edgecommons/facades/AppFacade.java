/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.facades;

import com.mbreissi.edgecommons.config.ConfigManager;
import com.mbreissi.edgecommons.messaging.Message;
import com.mbreissi.edgecommons.messaging.MessageBuilder;
import com.mbreissi.edgecommons.messaging.MessagingClient;
import com.mbreissi.edgecommons.messaging.Qos;
import com.mbreissi.edgecommons.uns.Uns;
import com.mbreissi.edgecommons.uns.UnsClass;
import com.google.gson.JsonObject;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.time.Duration;
import java.util.Objects;

/**
 * The {@code app()} publish facade — free-form inter-component pub/sub on the {@code app} class
 * (DESIGN-class-facades §2.3, D3). {@code app} is the intentionally-open class, so the facade's
 * value is <b>not</b> body enforcement (there is no contract to enforce) — it is removing the raw
 * three-line ritual and guaranteeing topic + identity correctness: a <b>named</b> header, the
 * developer body <b>verbatim</b>, minted onto {@code app/{channel}} with the envelope identity
 * stamped. {@code app} is non-reserved — this publishes through the ordinary guarded
 * {@code messaging().publish(...)}.
 *
 * <p><b>Routing:</b> {@link Channel#LOCAL} (default) or {@link Channel#NORTHBOUND}; a {@code stream}
 * route is <b>rejected</b> (same reasoning as {@code events()}).
 *
 * <p><b>Library-internal:</b> obtain via {@code gg.instance(id).app()} or the {@code main}
 * convenience {@code gg.getApp()}.
 */
public final class AppFacade {

    private static final Logger LOGGER = LogManager.getLogger(AppFacade.class);

    /** The app envelope header version (the header {@code name} is the caller's chosen name). */
    public static final String APP_MESSAGE_VERSION = "1.0";

    private final ConfigManager configManager;
    private final String instanceId;
    private final Uns uns;
    private final MessagingClient messaging;

    /**
     * Immutable prepared application publication. The encoded bytes are captured once with the
     * message UUID/timestamp and are defensively copied, allowing a durable outbox to retry the
     * exact envelope rather than reconstructing it.
     */
    public static final class PreparedAppMessage {
        private final String topic;
        private final Message message;
        private final byte[] encodedBytes;

        private PreparedAppMessage(String topic, Message message, byte[] encodedBytes) {
            this.topic = Objects.requireNonNull(topic, "topic must not be null");
            this.message = Objects.requireNonNull(message, "message must not be null");
            this.encodedBytes = Objects.requireNonNull(
                    encodedBytes, "encodedBytes must not be null").clone();
        }

        /** The facade-generated {@code app/{channel}} topic. */
        public String topic() {
            return topic;
        }

        /** The prepared envelope (including its stable UUID, timestamp, identity, and body). */
        public Message message() {
            return Message.fromBytes(encodedBytes);
        }

        /** Exact serialized envelope bytes; each call returns a defensive copy. */
        public byte[] encodedBytes() {
            return encodedBytes.clone();
        }
    }

    /**
     * Library-internal constructor (see class javadoc).
     *
     * @param configManager the component config manager (envelope identity)
     * @param instanceId    the instance token this facade is bound to
     * @param uns           the instance-bound UNS topic builder
     * @param messaging     the (guarded) messaging client
     */
    public AppFacade(ConfigManager configManager, String instanceId, Uns uns,
                     MessagingClient messaging) {
        this.configManager = Objects.requireNonNull(configManager, "configManager must not be null");
        this.instanceId = instanceId;   // D‑U28: null ⇒ component scope
        this.uns = Objects.requireNonNull(uns, "uns must not be null");
        this.messaging = Objects.requireNonNull(messaging, "messaging must not be null");
    }

    /**
     * Publishes a free-form message on {@code app/{channel}} locally.
     *
     * @param name    the envelope header {@code name} (the developer's message name; REQUIRED)
     * @param channel the {@code app/{channel}} tail (each {@code /}-token is sanitized; REQUIRED)
     * @param body    the developer body, published verbatim
     */
    public void publish(String name, String channel, JsonObject body) {
        publish(name, channel, body, null);
    }

    /**
     * {@link #publish(String, String, JsonObject)} with an explicit LOCAL/NORTHBOUND routing.
     *
     * @param name    the envelope header {@code name} (REQUIRED)
     * @param channel the {@code app/{channel}} tail (REQUIRED)
     * @param body    the developer body, published verbatim
     * @param routing the routing channel, or {@code null} for LOCAL
     * @throws IllegalArgumentException when {@code routing} is a {@code stream} channel
     */
    public void publish(String name, String channel, JsonObject body, Channel routing) {
        publish(prepare(name, channel, body), routing);
    }

    /**
     * Constructs an application message without publishing it. The returned bytes and envelope
     * are one immutable publication attempt suitable for durable outbox storage.
     */
    public PreparedAppMessage prepare(String name, String channel, JsonObject body) {
        return prepareInternal(name, channel, body, null);
    }

    /**
     * Prepares an application message carrying a received request's conversation correlation.
     *
     * @throws IllegalArgumentException when the request/header/correlation is absent
     */
    public PreparedAppMessage prepareCorrelated(String name, String channel, JsonObject body,
                                                Message request) {
        if (request == null || request.getHeader() == null) {
            throw new IllegalArgumentException(
                    "correlated app message requires a request with a header");
        }
        return prepareCorrelated(
                name, channel, body, request.getHeader().getCorrelationId());
    }

    /**
     * Prepares an application message with an explicit existing conversation correlation. The
     * correlation joins observations; it is not treated as an idempotency key.
     */
    public PreparedAppMessage prepareCorrelated(String name, String channel, JsonObject body,
                                                String correlationId) {
        if (correlationId == null || correlationId.isEmpty()) {
            throw new IllegalArgumentException(
                    "correlated app message requires a non-empty correlation id");
        }
        return prepareInternal(name, channel, body, correlationId);
    }

    private PreparedAppMessage prepareInternal(String name, String channel, JsonObject body,
                                               String correlationId) {
        if (name == null || name.isEmpty()) {
            throw new IllegalArgumentException("app publish requires a non-empty header name");
        }
        if (channel == null || channel.isEmpty()) {
            throw new IllegalArgumentException("app publish requires a non-empty channel");
        }
        String topic = uns.topic(UnsClass.APP, channelToken(channel));
        MessageBuilder builder = MessageBuilder.create(name, APP_MESSAGE_VERSION)
                .withConfig(configManager)
                .withInstance(instanceId)
                .withPayload(body);
        if (correlationId != null) {
            builder.withCorrelationId(correlationId);
        }
        Message msg = builder.build();
        return new PreparedAppMessage(topic, msg, msg.toBytes());
    }

    /** Publishes a previously prepared envelope through the existing immediate path. */
    public void publish(PreparedAppMessage prepared, Channel routing) {
        Objects.requireNonNull(prepared, "prepared must not be null");
        rejectStream(routing);
        if (routing != null && routing.kind() == Channel.Kind.NORTHBOUND) {
            try {
                messaging.publishNorthbound(
                        prepared.topic(), prepared.message(), Qos.AT_LEAST_ONCE);
            } catch (Exception e) {
                LOGGER.warn("Northbound app publish on '{}' failed (local readiness unaffected): {}",
                        prepared.topic(), e.toString());
            }
        } else {
            messaging.publish(prepared.topic(), prepared.message());
        }
    }

    /** Publishes a prepared envelope locally and waits for explicit QoS-1 confirmation. */
    public void publishConfirmed(PreparedAppMessage prepared, Duration timeout) {
        publishConfirmed(prepared, null, timeout);
    }

    /**
     * Publishes the exact bytes of a prepared envelope and waits for positive local/northbound
     * acknowledgement. Failures propagate so a durable outbox can leave the record pending.
     */
    public void publishConfirmed(PreparedAppMessage prepared, Channel routing, Duration timeout) {
        Objects.requireNonNull(prepared, "prepared must not be null");
        rejectStream(routing);
        if (routing != null && routing.kind() == Channel.Kind.NORTHBOUND) {
            messaging.publishNorthboundConfirmed(prepared.topic(), prepared.encodedBytes(),
                    Qos.AT_LEAST_ONCE, timeout);
        } else {
            messaging.publishConfirmed(prepared.topic(), prepared.encodedBytes(),
                    Qos.AT_LEAST_ONCE, timeout);
        }
    }

    /** The sanitized {@code app} channel token (each {@code /}-token → a UNS token). */
    private static String channelToken(String channel) {
        String[] tokens = channel.split("/", -1);
        StringBuilder sb = new StringBuilder(channel.length());
        for (int i = 0; i < tokens.length; i++) {
            if (i > 0) {
                sb.append('/');
            }
            sb.append(ConfigManager.sanitize(tokens[i]));
        }
        return sb.toString();
    }

    private static void rejectStream(Channel channel) {
        if (channel != null && channel.kind() == Channel.Kind.STREAM) {
            throw new IllegalArgumentException("app() does not support the stream channel -"
                    + " use data() for streamed telemetry");
        }
    }
}
