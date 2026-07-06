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
import com.google.gson.Gson;
import com.google.gson.JsonArray;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.nio.charset.StandardCharsets;
import java.time.Clock;
import java.time.Instant;
import java.util.Objects;

/**
 * The {@code data()} publish facade — the telemetry / signal data plane (DESIGN-class-facades §2.1,
 * D2/D5). It <b>constructs and validates the {@code SouthboundSignalUpdate} body</b>
 * ({@code device}/{@code signal}/{@code samples}) so an adapter never hand-builds it, applies the
 * body defaults, sanitizes the signal path into the UNS {@code data} channel, stamps the envelope
 * identity, and routes to the resolved {@link Channel}. It publishes through the <b>ordinary,
 * guarded</b> {@code messaging().publish(...)} — {@code data} is non-reserved, so it passes the
 * guard; the facade adds body-contract enforcement + defaults, <b>not</b> privilege (it is NOT a
 * {@code ReservedPublisher}).
 *
 * <p><b>Body ({@code header.name} = {@value #DATA_MESSAGE_NAME}, version {@value #DATA_MESSAGE_VERSION}):</b>
 * <pre>{@code
 *   { "device": {adapter, instance, endpoint}?,          // optional block
 *     "signal": { "id": <REQUIRED>, "name"?, "address"? },
 *     "samples": [ { "value": <REQUIRED>, "quality", "qualityRaw"?, "sourceTs"?, "serverTs" } ] }
 * }</pre>
 *
 * <p><b>Defaulting (DESIGN-class-facades §2.1, pinned by {@code uns-test-vectors/data.json}):</b>
 * <ol>
 *   <li>{@code quality} → {@link Quality#GOOD} when omitted on a sample that carries a value.</li>
 *   <li>{@code qualityRaw} → the synthetic marker {@value #QUALITY_UNSPECIFIED} when (and only
 *       when) the quality was defaulted; else the caller's value verbatim, else absent.</li>
 *   <li>{@code serverTs} → now (ISO-8601 UTC {@code …Z}, from the injected {@link Clock}) when
 *       omitted; {@code sourceTs} is <b>never</b> synthesized (absent when the source has none).</li>
 *   <li>the {@code samples} wrapper is enforced for the value-shorthand (a caller never emits a
 *       bare value).</li>
 *   <li>{@code signal.id} is the <b>only</b> hard reject — a publish with no stable id throws
 *       {@link IllegalArgumentException} at the call site.</li>
 * </ol>
 *
 * <p><b>Channel routing (DESIGN-class-facades §4, D1):</b> per-call {@link SignalUpdate.Builder#via}
 * override ▸ config {@code publish.channel} (instance ▸ global) ▸ {@link Channel#LOCAL}. A
 * {@code stream:<name>} route serializes the same envelope and appends it to
 * {@code getStreams().stream(name)} (partition key = {@code signal.id}, ts = {@code serverTs}); when
 * streaming is not configured it falls back to a LOCAL publish (readiness / no-streaming → local).
 * Northbound / stream transport failures are caught and logged — they must never flip local
 * readiness.
 *
 * <p><b>Library-internal:</b> obtain the bound instance via {@code gg.instance(id).data()} (or the
 * {@code main}-instance convenience {@code gg.getData()}); the public constructor exists only so the
 * per-instance handle in {@code com.mbreissi.edgecommons} can wire it.
 */
public final class DataFacade {

    private static final Logger LOGGER = LogManager.getLogger(DataFacade.class);

    /** The signal-update envelope header name ({@code docs/SOUTHBOUND.md} §2). */
    public static final String DATA_MESSAGE_NAME = "SouthboundSignalUpdate";
    /** The signal-update envelope header version. */
    public static final String DATA_MESSAGE_VERSION = "1.0";
    /** The {@code qualityRaw} marker written when {@code quality} was defaulted to {@code GOOD}. */
    public static final String QUALITY_UNSPECIFIED = "unspecified";

    private static final Gson GSON = new Gson();

    private final ConfigManager configManager;
    private final String instanceId;
    private final Uns uns;
    private final MessagingClient messaging;
    private final StreamSink streamSink; // nullable -> stream route falls back to local
    private final Clock clock;
    private volatile boolean warnedNoStream = false;

    /**
     * Library-internal constructor (see class javadoc). The identity-stamping and topic-minting
     * seams are pre-bound to one instance token.
     *
     * @param configManager the component config manager (envelope identity + {@code publish.channel})
     * @param instanceId    the instance token this facade is bound to
     * @param uns           the instance-bound UNS topic builder
     * @param messaging     the (guarded) messaging client
     * @param streamSink    the stream seam, or {@code null} when streaming is not configured
     * @param clock         the clock for {@code serverTs} defaults (injected for deterministic tests)
     */
    public DataFacade(ConfigManager configManager, String instanceId, Uns uns,
                      MessagingClient messaging, StreamSink streamSink, Clock clock) {
        this.configManager = Objects.requireNonNull(configManager, "configManager must not be null");
        this.instanceId = Objects.requireNonNull(instanceId, "instanceId must not be null");
        this.uns = Objects.requireNonNull(uns, "uns must not be null");
        this.messaging = Objects.requireNonNull(messaging, "messaging must not be null");
        this.streamSink = streamSink;
        this.clock = Objects.requireNonNull(clock, "clock must not be null");
    }

    // ===================== fluent builder entry points =====================

    /**
     * Starts building a {@code SouthboundSignalUpdate} for a stable {@code signal.id} — the fluent
     * body builder that subsumes the hand-assembled {@code JsonObject}. Terminate with
     * {@link SignalUpdate.Builder#publish()}.
     *
     * @param id the stable {@code signal.id} (REQUIRED — the consumer key)
     * @return the bound builder
     */
    public SignalUpdate.Builder signal(String id) {
        return new SignalUpdate.Builder(this, id);
    }

    // ===================== value shorthand =====================

    /**
     * The value-shorthand: publish one value for a signal path (the path doubles as the stable
     * {@code signal.id}). The single value is wrapped into a one-element {@code samples} array with
     * {@code quality=GOOD}, {@code qualityRaw="unspecified"}, {@code serverTs=now} — a caller never
     * emits a bare value.
     *
     * @param signalPath the signal path / stable id (e.g. {@code "press12/temperature"})
     * @param value      the measured value (REQUIRED)
     */
    public void publish(String signalPath, Object value) {
        signal(signalPath).addSample(value).signalPath(signalPath).publish();
    }

    /**
     * The value-shorthand with an explicit quality (so a source that knows the read is stale/failed
     * marks it {@code BAD}/{@code UNCERTAIN}).
     *
     * @param signalPath the signal path / stable id
     * @param value      the measured value (REQUIRED)
     * @param quality    the normalized quality
     */
    public void publish(String signalPath, Object value, Quality quality) {
        signal(signalPath).addSample(value, quality).signalPath(signalPath).publish();
    }

    // ===================== the raw escape hatch =====================

    /**
     * The raw escape hatch (D5): publishes a caller-owned pre-built body verbatim to
     * {@code data/{signalPath}}, applying <b>no</b> body defaulting — only the topic + identity
     * guarantees. For a component with an exotic body the facade should not shape.
     *
     * @param signalPath the signal path (sanitized into the channel)
     * @param body       the pre-built body, published untouched
     */
    public void publishBody(String signalPath, JsonObject body) {
        publishBody(signalPath, body, null);
    }

    /**
     * {@link #publishBody(String, JsonObject)} with an explicit {@link Channel} override.
     *
     * @param signalPath the signal path (sanitized into the channel)
     * @param body       the pre-built body, published untouched
     * @param via        the channel override, or {@code null} to resolve config ▸ LOCAL
     */
    public void publishBody(String signalPath, JsonObject body, Channel via) {
        Objects.requireNonNull(body, "body must not be null");
        String channel = channelToken(signalPath);
        String topic = uns.topic(UnsClass.DATA, channel);
        Message msg = message(body);
        route(via, topic, msg, signalPath, firstServerTsMillis(body));
    }

    // ===================== the SignalUpdate publish path =====================

    /**
     * Publishes a built {@link SignalUpdate}: validates {@code signal.id}, constructs the body with
     * the defaulting rules, sanitizes the path into the {@code data} channel, stamps the envelope,
     * and routes to the resolved channel.
     *
     * @param update the signal update
     * @throws IllegalArgumentException when {@code signal.id} is missing/empty or a sample carries
     *                                  no value, or the signal path is empty
     */
    public void publish(SignalUpdate update) {
        Objects.requireNonNull(update, "update must not be null");
        if (update.signalId() == null || update.signalId().isEmpty()) {
            throw new IllegalArgumentException("data publish requires a stable signal.id"
                    + " (the consumer key) - it is the only non-defaultable field");
        }
        if (update.samples().isEmpty()) {
            throw new IllegalArgumentException("data publish requires at least one sample");
        }
        JsonObject body = buildBody(update);
        String channel = channelToken(update.effectiveSignalPath());
        String topic = uns.topic(UnsClass.DATA, channel);
        Message msg = message(body);
        route(update.via(), topic, msg, update.signalId(), firstServerTsMillis(body));
    }

    // ===================== body construction (THE contract) =====================

    /**
     * Constructs the wire body from a {@link SignalUpdate}, applying the §2.1 defaulting rules
     * (quality → {@code GOOD} + {@code qualityRaw} marker, {@code serverTs} → now, samples wrapper).
     * Deterministic given the injected clock — this is the exact body the vectors pin.
     *
     * @param update the signal update (its {@code signal.id} must be set)
     * @return the constructed {@code SouthboundSignalUpdate} body
     * @throws IllegalArgumentException when a sample carries no value
     */
    public JsonObject buildBody(SignalUpdate update) {
        JsonObject signal = new JsonObject();
        signal.addProperty("id", update.signalId());
        if (update.signalName() != null) {
            signal.addProperty("name", update.signalName());
        }
        if (update.signalAddress() != null) {
            signal.add("address", update.signalAddress());
        }

        JsonArray samples = new JsonArray();
        for (SignalUpdate.Sample sample : update.samples()) {
            samples.add(buildSample(sample));
        }

        JsonObject body = new JsonObject();
        if (update.device() != null) {
            body.add("device", update.device());
        }
        body.add("signal", signal);
        body.add("samples", samples);
        return body;
    }

    /** Builds one sample with the quality/qualityRaw/serverTs defaulting rules. */
    private JsonObject buildSample(SignalUpdate.Sample sample) {
        if (sample.value() == null) {
            throw new IllegalArgumentException("data sample value is required (a quality-only"
                    + " sample is not a sample) - pass BAD/UNCERTAIN for a failed read");
        }
        JsonObject out = new JsonObject();
        out.add("value", toJsonElement(sample.value()));

        boolean qualityDefaulted = sample.quality() == null;
        Quality quality = qualityDefaulted ? Quality.GOOD : sample.quality();
        out.addProperty("quality", quality.wire());

        String qualityRaw = sample.qualityRaw();
        if (qualityRaw == null && qualityDefaulted) {
            qualityRaw = QUALITY_UNSPECIFIED;
        }
        if (qualityRaw != null) {
            out.addProperty("qualityRaw", qualityRaw);
        }

        if (sample.sourceTs() != null) {
            out.addProperty("sourceTs", sample.sourceTs());
        }
        out.addProperty("serverTs", sample.serverTs() != null ? sample.serverTs() : nowIso());
        return out;
    }

    // ===================== channel routing =====================

    /**
     * Resolves the effective channel: per-call {@code via} override ▸ config {@code publish.channel}
     * (instance ▸ global) ▸ {@link Channel#LOCAL} (DESIGN-class-facades §4, D1).
     *
     * @param via the per-call override, or {@code null}
     * @return the resolved channel (never {@code null})
     */
    public Channel resolveChannel(Channel via) {
        if (via != null) {
            return via;
        }
        Channel configured = configuredChannel();
        return configured != null ? configured : Channel.LOCAL;
    }

    /**
     * Reads the config {@code publish.channel} default (Option C): the bound instance's
     * {@code publish.channel} ▸ the global {@code component.global.publish.channel}. Best-effort —
     * any lookup/parse anomaly yields {@code null} (fall through to LOCAL).
     */
    private Channel configuredChannel() {
        try {
            Channel fromInstance = publishChannelOf(configManager.getInstanceConfig(instanceId));
            if (fromInstance != null) {
                return fromInstance;
            }
            return publishChannelOf(configManager.getGlobalConfig());
        } catch (RuntimeException e) {
            LOGGER.debug("publish.channel lookup failed (defaulting to LOCAL): {}", e.toString());
            return null;
        }
    }

    /** {@code section.publish.channel} as a {@link Channel}, or {@code null} when absent/unparseable. */
    private static Channel publishChannelOf(JsonObject section) {
        if (section == null || !section.has("publish") || !section.get("publish").isJsonObject()) {
            return null;
        }
        JsonObject publish = section.getAsJsonObject("publish");
        if (!publish.has("channel") || !publish.get("channel").isJsonPrimitive()) {
            return null;
        }
        return Channel.fromConfig(publish.get("channel").getAsString());
    }

    /**
     * Routes a built envelope to the resolved channel. LOCAL publishes on the guarded bus;
     * NORTHBOUND publishes to IoT Core; a stream route appends the serialized envelope to the named
     * stream (falling back to LOCAL when no sink is wired). Northbound / stream failures are
     * caught + logged (they must never flip local readiness).
     */
    private void route(Channel via, String topic, Message msg, String partitionKey, long tsMillis) {
        Channel channel = resolveChannel(via);
        switch (channel.kind()) {
            case LOCAL -> messaging.publish(topic, msg);
            case NORTHBOUND -> {
                try {
                    messaging.publishNorthbound(topic, msg, Qos.AT_LEAST_ONCE);
                } catch (Exception e) {
                    LOGGER.warn("Northbound data publish on '{}' failed (local readiness"
                            + " unaffected): {}", topic, e.toString());
                }
            }
            case STREAM -> appendToStream(channel.streamName(), topic, msg, partitionKey, tsMillis);
        }
    }

    /** The {@code stream:<name>} route: append the serialized envelope, or fall back to LOCAL. */
    private void appendToStream(String streamName, String topic, Message msg, String partitionKey,
                                long tsMillis) {
        if (streamSink == null) {
            if (!warnedNoStream) {
                warnedNoStream = true;
                LOGGER.warn("data channel 'stream:{}' requested but streaming is not configured -"
                        + " routing to LOCAL instead (readiness/no-streaming -> local)", streamName);
            }
            messaging.publish(topic, msg);
            return;
        }
        try {
            byte[] payload = msg.toDict().toString().getBytes(StandardCharsets.UTF_8);
            streamSink.append(streamName, partitionKey, tsMillis, payload);
        } catch (Exception e) {
            LOGGER.warn("Stream append to 'stream:{}' failed (local readiness unaffected): {}",
                    streamName, e.toString());
        }
    }

    // ===================== helpers =====================

    /** The sanitized channel token for a signal path (each {@code /}-token → a UNS token). */
    public String channelToken(String signalPath) {
        if (signalPath == null || signalPath.isEmpty()) {
            throw new IllegalArgumentException("data signal path must be non-empty");
        }
        String[] tokens = signalPath.split("/", -1);
        StringBuilder sb = new StringBuilder(signalPath.length());
        for (int i = 0; i < tokens.length; i++) {
            if (i > 0) {
                sb.append('/');
            }
            sb.append(ConfigManager.sanitize(tokens[i]));
        }
        return sb.toString();
    }

    /** Builds the identity-stamped envelope with the signal-update header. */
    private Message message(JsonObject body) {
        return MessageBuilder.create(DATA_MESSAGE_NAME, DATA_MESSAGE_VERSION)
                .withConfig(configManager)
                .withInstance(instanceId)
                .withPayload(body)
                .build();
    }

    /** ISO-8601 UTC ({@code …Z}) "now" from the injected clock. */
    private String nowIso() {
        return Instant.now(clock).toString();
    }

    /** The first sample's {@code serverTs} as epoch millis (the stream record timestamp). */
    private static long firstServerTsMillis(JsonObject body) {
        try {
            JsonArray samples = body.getAsJsonArray("samples");
            if (samples != null && !samples.isEmpty()) {
                JsonObject first = samples.get(0).getAsJsonObject();
                if (first.has("serverTs")) {
                    return Instant.parse(first.get("serverTs").getAsString()).toEpochMilli();
                }
            }
        } catch (RuntimeException ignored) {
            // Fall through to now() below.
        }
        return System.currentTimeMillis();
    }

    /** A JSON-native value: a {@link JsonElement} verbatim, else via Gson's reflective adapter. */
    private static JsonElement toJsonElement(Object value) {
        if (value instanceof JsonElement element) {
            return element;
        }
        return GSON.toJsonTree(value);
    }

    /** The instance token this facade is bound to. */
    public String instanceId() {
        return instanceId;
    }
}
