/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.facades;

import com.mbreissi.ggcommons.config.ConfigManager;
import com.mbreissi.ggcommons.messaging.Message;
import com.mbreissi.ggcommons.messaging.MessageBuilder;
import com.mbreissi.ggcommons.messaging.MessagingClient;
import com.mbreissi.ggcommons.uns.Uns;
import com.mbreissi.ggcommons.uns.UnsClass;
import com.google.gson.JsonObject;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import software.amazon.awssdk.aws.greengrass.model.QOS;

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
        this.instanceId = Objects.requireNonNull(instanceId, "instanceId must not be null");
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
        if (name == null || name.isEmpty()) {
            throw new IllegalArgumentException("app publish requires a non-empty header name");
        }
        if (channel == null || channel.isEmpty()) {
            throw new IllegalArgumentException("app publish requires a non-empty channel");
        }
        rejectStream(routing);
        String topic = uns.topic(UnsClass.APP, channelToken(channel));
        Message msg = MessageBuilder.create(name, APP_MESSAGE_VERSION)
                .withConfig(configManager)
                .withInstance(instanceId)
                .withPayload(body)
                .build();
        if (routing != null && routing.kind() == Channel.Kind.NORTHBOUND) {
            try {
                messaging.publishToIoTCore(topic, msg, QOS.AT_LEAST_ONCE);
            } catch (Exception e) {
                LOGGER.warn("Northbound app publish on '{}' failed (local readiness unaffected): {}",
                        topic, e.toString());
            }
        } else {
            messaging.publish(topic, msg);
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
