/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.facades;

import java.util.Objects;

/**
 * A publish-channel address (DESIGN-class-facades §4, {@code DESIGN-channels.md}): the uniform
 * {@code { local, northbound, stream:<name> }} routing target the publish facades resolve on.
 *
 * <ul>
 *   <li>{@link #LOCAL} — the local/IPC bus ({@code messaging().publish}). The default.</li>
 *   <li>{@link #NORTHBOUND} — the northbound/cloud broker ({@code messaging().publishNorthbound}).</li>
 *   <li>{@link #stream(String)} — the named durable telemetry stream
 *       ({@code getStreams().stream(name).append(...)}); <b>only {@link DataFacade} honors it</b>
 *       — {@code events()}/{@code app()} reject a stream channel (they are low-rate control-plane,
 *       not bulk telemetry).</li>
 * </ul>
 *
 * <p>Modeled as a value class rather than a bare enum because the {@code stream} target carries a
 * stream name. {@link #fromConfig(String)} parses the config {@code publish.channel} string
 * (Option C, DESIGN-class-facades §4): {@code "local"}, {@code "northbound"}, or
 * {@code "stream:<name>"}.
 *
 * <p><b>Mirror note (Python/Rust/TS):</b> a small tagged union with the same three variants and the
 * same {@code fromConfig} parse.
 */
public final class Channel {

    /** The routing kind. */
    public enum Kind { LOCAL, NORTHBOUND, STREAM }

    /** The local/IPC bus channel (the default). */
    public static final Channel LOCAL = new Channel(Kind.LOCAL, null);

    /** The northbound/cloud channel. */
    public static final Channel NORTHBOUND = new Channel(Kind.NORTHBOUND, null);

    private final Kind kind;
    private final String streamName; // non-null iff kind == STREAM

    private Channel(Kind kind, String streamName) {
        this.kind = kind;
        this.streamName = streamName;
    }

    /**
     * The named-durable-stream channel.
     *
     * @param name the stream name (must match a configured {@code streaming.streams[].name})
     * @return the stream channel
     * @throws IllegalArgumentException if {@code name} is null or empty
     */
    public static Channel stream(String name) {
        if (name == null || name.isEmpty()) {
            throw new IllegalArgumentException("stream channel name must be non-empty");
        }
        return new Channel(Kind.STREAM, name);
    }

    /** The routing kind. */
    public Kind kind() {
        return kind;
    }

    /** The stream name (non-null only for a {@link Kind#STREAM} channel). */
    public String streamName() {
        return streamName;
    }

    /**
     * Parses a config {@code publish.channel} string into a channel (DESIGN-class-facades §4,
     * Option C). Recognized: {@code "local"} → {@link #LOCAL}; {@code "northbound"} →
     * {@link #NORTHBOUND}; {@code "stream:<name>"} →
     * {@link #stream(String)}. Any other (or null/empty) value yields {@code null} so the caller
     * can fall through to its own default.
     *
     * @param value the raw config string (may be {@code null})
     * @return the parsed channel, or {@code null} when unrecognized/absent
     */
    public static Channel fromConfig(String value) {
        if (value == null) {
            return null;
        }
        String v = value.trim();
        if (v.isEmpty()) {
            return null;
        }
        String lower = v.toLowerCase(java.util.Locale.ROOT);
        if (lower.equals("local")) {
            return LOCAL;
        }
        if (lower.equals("northbound")) {
            return NORTHBOUND;
        }
        if (lower.startsWith("stream:")) {
            String name = v.substring("stream:".length());
            return name.isEmpty() ? null : stream(name);
        }
        return null;
    }

    @Override
    public boolean equals(Object o) {
        if (this == o) {
            return true;
        }
        if (!(o instanceof Channel other)) {
            return false;
        }
        return kind == other.kind && Objects.equals(streamName, other.streamName);
    }

    @Override
    public int hashCode() {
        return Objects.hash(kind, streamName);
    }

    /** {@code "local"} / {@code "northbound"} / {@code "stream:<name>"} — the config-string form. */
    @Override
    public String toString() {
        return switch (kind) {
            case LOCAL -> "local";
            case NORTHBOUND -> "northbound";
            case STREAM -> "stream:" + streamName;
        };
    }
}
