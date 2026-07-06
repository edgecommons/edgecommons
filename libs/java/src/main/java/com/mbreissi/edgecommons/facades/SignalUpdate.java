/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.facades;

import com.google.gson.JsonObject;

import java.util.ArrayList;
import java.util.List;

/**
 * The constructed {@code SouthboundSignalUpdate} body (DESIGN-class-facades §2.1,
 * {@code docs/SOUTHBOUND.md} §2) — the value object that <b>replaces the adapters' hand-assembled
 * {@code JsonObject}</b>. It holds the raw inputs (optional {@code device} block; the {@code signal}
 * with its REQUIRED stable {@code id} plus optional {@code name}/{@code address}; the {@code samples};
 * the sanitized-into-a-channel {@code signalPath}; an optional per-call {@link Channel} override).
 * {@link DataFacade#buildBody(SignalUpdate)} applies the defaulting rules (quality → {@code GOOD},
 * {@code serverTs} → now, the {@code samples} wrapper) and produces the wire body.
 *
 * <p>Obtain a builder from {@link DataFacade#signal(String)} and terminate with
 * {@link Builder#publish()} (or {@link Builder#build()} for the {@link DataFacade#publish(SignalUpdate)}
 * form). The {@code signal.id} is the only structural requirement — a missing one is a fail-fast
 * {@link IllegalArgumentException} at publish (DESIGN-class-facades §5.2), never a dropped message.
 *
 * <p><b>Mirror note (Python/Rust/TS):</b> the same builder shape; {@code Sample} is a small
 * value/record with the same five fields (value REQUIRED, the rest optional).
 */
public final class SignalUpdate {

    /**
     * One sample: a measured {@code value} (REQUIRED) plus the optional quality/timestamp parts.
     * A {@code null} {@code quality} is defaulted to {@link Quality#GOOD} by the facade; a
     * {@code null} {@code serverTs} is filled with now; {@code sourceTs} is never synthesized;
     * {@code qualityRaw} is a synthetic {@code "unspecified"} marker when (and only when) the
     * quality was defaulted, else passed through verbatim.
     *
     * @param value      the measured value (JSON-native: number/boolean/string/array/JsonElement) —
     *                   REQUIRED (a null value is a fail-fast error at build)
     * @param quality    the normalized quality, or {@code null} to default to {@link Quality#GOOD}
     * @param qualityRaw the native status code, or {@code null}
     * @param sourceTs   the device/field ISO-8601 timestamp, or {@code null} (never synthesized)
     * @param serverTs   the protocol-server ISO-8601 timestamp, or {@code null} to default to now
     */
    public record Sample(Object value, Quality quality, String qualityRaw, String sourceTs,
                         String serverTs) {

        /** A value-only sample: quality defaults to {@code GOOD}, {@code serverTs} to now. */
        public static Sample of(Object value) {
            return new Sample(value, null, null, null, null);
        }

        /** A value + explicit quality sample ({@code serverTs} defaults to now). */
        public static Sample of(Object value, Quality quality) {
            return new Sample(value, quality, null, null, null);
        }

        /** A value + quality + device timestamp sample ({@code serverTs} defaults to now). */
        public static Sample of(Object value, Quality quality, String sourceTs) {
            return new Sample(value, quality, null, sourceTs, null);
        }
    }

    private final JsonObject device;      // nullable
    private final String signalId;        // required (validated at publish)
    private final String signalName;      // nullable
    private final JsonObject signalAddress; // nullable
    private final List<Sample> samples;   // never null
    private final String signalPath;      // nullable -> defaults to signalId
    private final Channel via;            // nullable per-call override

    private SignalUpdate(Builder b) {
        this.device = b.device;
        this.signalId = b.signalId;
        this.signalName = b.signalName;
        this.signalAddress = b.signalAddress;
        this.samples = List.copyOf(b.samples);
        this.signalPath = b.signalPath;
        this.via = b.via;
    }

    /** The optional {@code device} block ({@code {adapter, instance, endpoint}}), or {@code null}. */
    public JsonObject device() {
        return device;
    }

    /** The stable {@code signal.id} (REQUIRED; the consumer key). */
    public String signalId() {
        return signalId;
    }

    /** The human {@code signal.name}, or {@code null}. */
    public String signalName() {
        return signalName;
    }

    /** The protocol-native {@code signal.address}, or {@code null}. */
    public JsonObject signalAddress() {
        return signalAddress;
    }

    /** The samples (never null; may be empty — the facade rejects an empty list at publish). */
    public List<Sample> samples() {
        return samples;
    }

    /** The channel path (the {@code data/{signalPath}} tail); {@code null} means "use signalId". */
    public String signalPath() {
        return signalPath;
    }

    /** The effective channel path: {@link #signalPath()} when set, else {@link #signalId()}. */
    public String effectiveSignalPath() {
        return signalPath != null ? signalPath : signalId;
    }

    /** The per-call {@link Channel} override, or {@code null} (resolve config default ▸ LOCAL). */
    public Channel via() {
        return via;
    }

    /**
     * The fluent {@code SouthboundSignalUpdate} builder — {@code signal(id).name(n).address(a)
     * .device(...).addSample(...).signalPath(p).publish()}. Reused across all four languages.
     */
    public static final class Builder {

        private final DataFacade facade; // nullable (null when built for the publish(update) form)
        private JsonObject device;
        private String signalId;
        private String signalName;
        private JsonObject signalAddress;
        private final List<Sample> samples = new ArrayList<>();
        private String signalPath;
        private Channel via;

        /** Detached builder (no facade) — terminate with {@link #build()}. */
        public Builder(String signalId) {
            this(null, signalId);
        }

        /** Facade-bound builder — terminate with {@link #publish()} or {@link #build()}. */
        Builder(DataFacade facade, String signalId) {
            this.facade = facade;
            this.signalId = signalId;
        }

        /** Sets the human {@code signal.name}. */
        public Builder name(String name) {
            this.signalName = name;
            return this;
        }

        /** Sets the protocol-native {@code signal.address}. */
        public Builder address(JsonObject address) {
            this.signalAddress = address;
            return this;
        }

        /** Sets the {@code device} block from its three parts (any may be {@code null}). */
        public Builder device(String adapter, String instance, String endpoint) {
            JsonObject d = new JsonObject();
            if (adapter != null) {
                d.addProperty("adapter", adapter);
            }
            if (instance != null) {
                d.addProperty("instance", instance);
            }
            if (endpoint != null) {
                d.addProperty("endpoint", endpoint);
            }
            this.device = d;
            return this;
        }

        /** Sets a pre-built {@code device} block. */
        public Builder device(JsonObject device) {
            this.device = device;
            return this;
        }

        /** Appends a value-only sample (quality defaults to {@code GOOD}, {@code serverTs} to now). */
        public Builder addSample(Object value) {
            samples.add(Sample.of(value));
            return this;
        }

        /** Appends a value + quality sample. */
        public Builder addSample(Object value, Quality quality) {
            samples.add(Sample.of(value, quality));
            return this;
        }

        /** Appends a value + quality + device-timestamp sample. */
        public Builder addSample(Object value, Quality quality, String sourceTs) {
            samples.add(Sample.of(value, quality, sourceTs));
            return this;
        }

        /** Appends a fully-specified sample. */
        public Builder addSample(Sample sample) {
            samples.add(sample);
            return this;
        }

        /** Appends a batch of samples (the coalesced-publish path). */
        public Builder addSamples(List<Sample> more) {
            samples.addAll(more);
            return this;
        }

        /**
         * Sets the channel path — the {@code data/{signalPath}} tail (each {@code /}-separated
         * token is sanitized into a UNS token by the facade). When unset, the stable
         * {@link #signalId} is used as the path (D-U15's sanitized-path-vs-stable-id split still
         * holds — the body's raw id rides untouched).
         */
        public Builder signalPath(String signalPath) {
            this.signalPath = signalPath;
            return this;
        }

        /** Sets a per-call {@link Channel} override (LOCAL / NORTHBOUND / stream). */
        public Builder via(Channel channel) {
            this.via = channel;
            return this;
        }

        /** Builds the immutable {@link SignalUpdate} (for the {@code publish(update)} form). */
        public SignalUpdate build() {
            return new SignalUpdate(this);
        }

        /**
         * Builds and publishes through the originating facade.
         *
         * @throws IllegalStateException if this builder was created detached (no facade) — use
         *                               {@link #build()} + {@link DataFacade#publish(SignalUpdate)}
         */
        public void publish() {
            if (facade == null) {
                throw new IllegalStateException("this SignalUpdate.Builder is detached - call"
                        + " build() and pass it to DataFacade.publish(SignalUpdate)");
            }
            facade.publish(build());
        }
    }
}
