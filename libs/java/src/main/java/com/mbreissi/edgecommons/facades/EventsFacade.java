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

import java.time.Clock;
import java.time.Instant;
import java.util.Objects;

/**
 * The {@code events()} publish facade — operator events &amp; alarms on the {@code evt} class
 * (DESIGN-class-facades §2.2, D8). It is the facade that <b>stops the §1.2 {@code evt} drift</b>: it
 * makes the {@code evt/{severity}/{type}} channel and the body shape non-negotiable by <b>deriving
 * the channel from the body's own {@code severity} + {@code type}</b>, so the topic and body can
 * never disagree (today each adapter sets them independently). {@code evt} is non-reserved — this
 * publishes through the ordinary guarded {@code messaging().publish(...)}.
 *
 * <p><b>Body ({@code header.name} = {@value #EVT_MESSAGE_NAME}, version {@value #EVT_MESSAGE_VERSION}):</b>
 * <pre>{@code
 *   { "severity": "critical|warning|info|debug",  // REQUIRED (channel token 1)
 *     "type":      <REQUIRED>,                     // the event type (channel token 2, sanitized)
 *     "message":   <str>?,                         // optional operator text
 *     "timestamp": <iso>,                          // DEFAULTED to now
 *     "context":   { }?,                           // optional structured data
 *     "alarm":     <bool>?,  "active": <bool>? }   // present only for raiseAlarm/clearAlarm
 * }</pre>
 *
 * <p><b>Channel:</b> {@code evt/{severity.wire()}/{sanitize(type)}} (2 tokens). <b>Routing:</b>
 * {@link Channel#LOCAL} (default) or {@link Channel#NORTHBOUND} via {@link #via(Channel)} — alarms
 * often go straight to the cloud control plane. A {@code stream} route is <b>rejected</b> (events
 * are low-rate control-plane, not bulk telemetry).
 *
 * <p><b>Library-internal:</b> obtain via {@code gg.instance(id).events()} or the {@code main}
 * convenience {@code gg.getEvents()}.
 */
public final class EventsFacade {

    private static final Logger LOGGER = LogManager.getLogger(EventsFacade.class);

    /** The event envelope header name. */
    public static final String EVT_MESSAGE_NAME = "evt";
    /** The event envelope header version. */
    public static final String EVT_MESSAGE_VERSION = "1.0";

    private final ConfigManager configManager;
    private final String instanceId;
    private final Uns uns;
    private final MessagingClient messaging;
    private final Clock clock;
    private final Channel override; // nullable per-view channel override

    /**
     * Library-internal constructor (see class javadoc).
     *
     * @param configManager the component config manager (envelope identity)
     * @param instanceId    the instance token this facade is bound to
     * @param uns           the instance-bound UNS topic builder
     * @param messaging     the (guarded) messaging client
     * @param clock         the clock for the {@code timestamp} default (injected for tests)
     */
    public EventsFacade(ConfigManager configManager, String instanceId, Uns uns,
                        MessagingClient messaging, Clock clock) {
        this(configManager, instanceId, uns, messaging, clock, null);
    }

    private EventsFacade(ConfigManager configManager, String instanceId, Uns uns,
                         MessagingClient messaging, Clock clock, Channel override) {
        this.configManager = Objects.requireNonNull(configManager, "configManager must not be null");
        this.instanceId = Objects.requireNonNull(instanceId, "instanceId must not be null");
        this.uns = Objects.requireNonNull(uns, "uns must not be null");
        this.messaging = Objects.requireNonNull(messaging, "messaging must not be null");
        this.clock = Objects.requireNonNull(clock, "clock must not be null");
        this.override = override;
    }

    /**
     * Returns a channel-bound view for a per-call routing override (LOCAL or NORTHBOUND).
     *
     * @param channel the routing channel
     * @return a bound view whose {@code emit}/{@code raiseAlarm}/{@code clearAlarm} route there
     * @throws IllegalArgumentException when {@code channel} is a {@code stream} channel
     */
    public EventsFacade via(Channel channel) {
        rejectStream(channel);
        return new EventsFacade(configManager, instanceId, uns, messaging, clock, channel);
    }

    // ===================== emit =====================

    /**
     * Emits a one-shot event with an explicit severity, message, and structured context.
     *
     * @param severity the severity (channel token 1; REQUIRED)
     * @param type     the event type (channel token 2; REQUIRED)
     * @param message  optional operator text (may be {@code null})
     * @param context  optional structured data (may be {@code null})
     */
    public void emit(Severity severity, String type, String message, JsonObject context) {
        Objects.requireNonNull(severity, "severity must not be null");
        publish(severity, type, message, context, null, null);
    }

    /** {@link #emit(Severity, String, String, JsonObject)} with no context. */
    public void emit(Severity severity, String type, String message) {
        emit(severity, type, message, null);
    }

    /** Message-only convenience — severity defaults to {@link Severity#INFO}. */
    public void emit(String type, String message) {
        emit(Severity.INFO, type, message, null);
    }

    // ===================== alarms =====================

    /**
     * Raises a stateful alarm ({@code alarm=true, active=true}). Severity defaults to
     * {@link Severity#CRITICAL} so raises and clears of the same alarm ride the same
     * {@code evt/critical/{type}} channel (subsumes OPC UA's {@code connection-lost}).
     *
     * @param type    the alarm type (channel token 2)
     * @param message optional operator text
     * @param context optional structured data
     */
    public void raiseAlarm(String type, String message, JsonObject context) {
        raiseAlarm(Severity.CRITICAL, type, message, context);
    }

    /** {@link #raiseAlarm(String, String, JsonObject)} with an explicit severity. */
    public void raiseAlarm(Severity severity, String type, String message, JsonObject context) {
        Objects.requireNonNull(severity, "severity must not be null");
        publish(severity, type, message, context, true, true);
    }

    /**
     * Clears a stateful alarm ({@code alarm=true, active=false}). Severity defaults to
     * {@link Severity#CRITICAL} so the clear tracks on the same channel as the raise (subsumes OPC
     * UA's {@code connection-restored}).
     *
     * @param type    the alarm type (must match the raise's type)
     * @param context optional structured data
     */
    public void clearAlarm(String type, JsonObject context) {
        clearAlarm(Severity.CRITICAL, type, context);
    }

    /** {@link #clearAlarm(String, JsonObject)} with an explicit severity. */
    public void clearAlarm(Severity severity, String type, JsonObject context) {
        Objects.requireNonNull(severity, "severity must not be null");
        publish(severity, type, null, context, true, false);
    }

    // ===================== body construction + routing =====================

    /**
     * Constructs the {@code evt} wire body — the exact body the vectors pin. Deterministic given the
     * injected clock. Member order: severity, type, message?, timestamp, context?, alarm?, active?.
     *
     * @param severity the severity (REQUIRED)
     * @param type     the event type (REQUIRED, non-empty)
     * @param message  optional operator text
     * @param context  optional structured data
     * @param alarm    {@code true}/{@code false} for an alarm raise/clear; {@code null} for a plain event
     * @param active   the alarm active flag (only when {@code alarm != null})
     * @return the constructed {@code evt} body
     */
    public JsonObject buildBody(Severity severity, String type, String message, JsonObject context,
                               Boolean alarm, Boolean active) {
        if (type == null || type.isEmpty()) {
            throw new IllegalArgumentException("evt requires a non-empty type (it is a channel"
                    + " token and the event's kind)");
        }
        JsonObject body = new JsonObject();
        body.addProperty("severity", severity.wire());
        body.addProperty("type", type);
        if (message != null) {
            body.addProperty("message", message);
        }
        body.addProperty("timestamp", Instant.now(clock).toString());
        if (context != null) {
            body.add("context", context);
        }
        if (alarm != null) {
            body.addProperty("alarm", alarm);
            body.addProperty("active", active);
        }
        return body;
    }

    /** The {@code evt/{severity}/{type}} channel derived from the body's own severity + type. */
    public String channelFor(Severity severity, String type) {
        if (type == null || type.isEmpty()) {
            throw new IllegalArgumentException("evt requires a non-empty type");
        }
        return severity.wire() + "/" + ConfigManager.sanitize(type);
    }

    private void publish(Severity severity, String type, String message, JsonObject context,
                         Boolean alarm, Boolean active) {
        JsonObject body = buildBody(severity, type, message, context, alarm, active);
        String channel = channelFor(severity, type);
        String topic = uns.topic(UnsClass.EVT, channel);
        Message msg = MessageBuilder.create(EVT_MESSAGE_NAME, EVT_MESSAGE_VERSION)
                .withConfig(configManager)
                .withInstance(instanceId)
                .withPayload(body)
                .build();
        route(topic, msg);
    }

    /** LOCAL (default) or NORTHBOUND; a stream override is rejected up front by {@link #via}. */
    private void route(String topic, Message msg) {
        Channel channel = override != null ? override : Channel.LOCAL;
        if (channel.kind() == Channel.Kind.NORTHBOUND) {
            try {
                messaging.publishNorthbound(topic, msg, Qos.AT_LEAST_ONCE);
            } catch (Exception e) {
                LOGGER.warn("Northbound evt publish on '{}' failed (local readiness unaffected): {}",
                        topic, e.toString());
            }
        } else {
            messaging.publish(topic, msg);
        }
    }

    private static void rejectStream(Channel channel) {
        if (channel != null && channel.kind() == Channel.Kind.STREAM) {
            throw new IllegalArgumentException("events() does not support the stream channel -"
                    + " events are low-rate control-plane, not bulk telemetry (use data() for"
                    + " streamed telemetry)");
        }
    }
}
