/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.breissinger.ggcommons.streaming;

import com.google.gson.JsonArray;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;

import java.lang.foreign.Arena;
import java.lang.foreign.MemorySegment;
import java.util.ArrayList;
import java.util.List;

import static java.lang.foreign.ValueLayout.ADDRESS;
import static java.lang.foreign.ValueLayout.JAVA_LONG;

/**
 * Java handle to the native {@code ggstreamlog} {@code StreamService}: opens/recovers every stream
 * declared in a {@code streaming} config document, runs their background export engines, and hands
 * out {@link StreamHandle}s for producers.
 *
 * <p>This binds the shared Rust core over the C ABI (Panama), so Java components get the same
 * durable store-and-forward streaming as the Rust library, with the same config schema. Mirrors
 * {@code gg.streams()} in the Rust lib.
 *
 * <p>{@link AutoCloseable}: {@link #close()} flushes every buffer and stops the engines.
 */
public final class StreamService implements AutoCloseable {

    private final GgStreamNative n;
    private volatile MemorySegment service; // null after close
    private final String configJson;

    private StreamService(GgStreamNative n, MemorySegment service, String configJson) {
        this.n = n;
        this.service = service;
        this.configJson = configJson;
    }

    /**
     * Open every stream in {@code configJson} — the {@code streaming} section
     * ({@code {"streams":[{"name","sink","buffer","batch","delivery"}, ...]}}). Templates must be
     * resolved by the caller. Loads the native library on first use.
     *
     * @throws GgStreamException if the config is invalid or a stream fails to open
     */
    public static StreamService open(String configJson) {
        GgStreamNative n = GgStreamNative.instance();
        try (Arena a = Arena.ofConfined()) {
            MemorySegment cfg = a.allocateFrom(configJson);
            MemorySegment out = a.allocate(ADDRESS);
            MemorySegment err = a.allocate(ADDRESS);
            int rc = n.open(cfg, out, err);
            if (rc != GgStreamException.OK) {
                throw new GgStreamException(rc, n.takeError(err));
            }
            // The service is a native-heap pointer; it outlives this confined arena.
            return new StreamService(n, out.get(ADDRESS, 0), configJson);
        }
    }

    /**
     * A handle to the named stream for appending. The returned handle is independent of this
     * service's lifetime (it stays usable for append/flush even after {@link #close()}).
     *
     * @throws GgStreamException with {@link GgStreamException#ERR_UNKNOWN_STREAM} if not configured
     */
    public StreamHandle stream(String name) {
        MemorySegment svc = requireOpen();
        try (Arena a = Arena.ofConfined()) {
            MemorySegment nameSeg = a.allocateFrom(name);
            MemorySegment out = a.allocate(ADDRESS);
            MemorySegment err = a.allocate(ADDRESS);
            int rc = n.streamGet(svc, nameSeg, out, err);
            if (rc != GgStreamException.OK) {
                throw new GgStreamException(rc, n.takeError(err));
            }
            return new StreamHandle(n, out.get(ADDRESS, 0), name);
        }
    }

    /**
     * A stats snapshot for the named stream.
     *
     * @throws GgStreamException with {@link GgStreamException#ERR_UNKNOWN_STREAM} if not configured
     */
    public StreamStats stats(String name) {
        MemorySegment svc = requireOpen();
        try (Arena a = Arena.ofConfined()) {
            MemorySegment nameSeg = a.allocateFrom(name);
            MemorySegment st = a.allocate(JAVA_LONG.byteSize() * 10);
            int rc = n.stats(svc, nameSeg, st);
            if (rc != GgStreamException.OK) {
                throw new GgStreamException(rc, null);
            }
            return new StreamStats(
                    st.getAtIndex(JAVA_LONG, 0),
                    st.getAtIndex(JAVA_LONG, 1),
                    st.getAtIndex(JAVA_LONG, 2),
                    st.getAtIndex(JAVA_LONG, 3),
                    st.getAtIndex(JAVA_LONG, 4),
                    st.getAtIndex(JAVA_LONG, 5),
                    st.getAtIndex(JAVA_LONG, 6),
                    st.getAtIndex(JAVA_LONG, 7),
                    st.getAtIndex(JAVA_LONG, 8),
                    st.getAtIndex(JAVA_LONG, 9));
        }
    }

    /**
     * Register the host {@link SinkFunction} that drains {@code callback}-type streams (the
     * "bring-your-own-sink" extension; CloudWatch's durable buffer is the first in-tree consumer).
     * Must be called <em>before</em> {@link #open} — the core binds the callback per stream at open
     * time, so a callback stream opened with no sink registered stays buffer-only (records persist
     * but are not exported) until reopened. Installs the native upcall on first use; the function
     * itself may be replaced freely. Run with {@code --enable-native-access=ALL-UNNAMED}.
     *
     * @param sink the host sink invoked per export batch on the engine thread (thread-safe, prompt)
     */
    public static void registerSink(SinkFunction sink) {
        SinkBridge.setSink(sink);
    }

    /** Clear the registered sink — subsequent batches become retryable failures (held on disk). */
    public static void unregisterSink() {
        SinkBridge.clearSink();
    }

    /**
     * Whether the native {@code ggstreamlog} library can be loaded (so durable streaming is
     * available). Returns {@code false} instead of throwing if the cdylib is missing — callers can
     * fall back to a non-durable path. Mainly for tests / capability probes.
     */
    public static boolean nativeAvailable() {
        try {
            GgStreamNative.instance();
            return true;
        } catch (Throwable t) {
            return false;
        }
    }

    /** The config document this service was opened with. */
    public String configJson() {
        return configJson;
    }

    /** The stream names declared in a {@code streaming} config document (empty if none/invalid). */
    public static List<String> streamNames(String configJson) {
        List<String> names = new ArrayList<>();
        try {
            JsonElement root = JsonParser.parseString(configJson);
            if (!root.isJsonObject()) {
                return names;
            }
            JsonObject obj = root.getAsJsonObject();
            if (!obj.has("streams") || !obj.get("streams").isJsonArray()) {
                return names;
            }
            JsonArray streams = obj.getAsJsonArray("streams");
            for (JsonElement e : streams) {
                if (e.isJsonObject() && e.getAsJsonObject().has("name")) {
                    names.add(e.getAsJsonObject().get("name").getAsString());
                }
            }
        } catch (RuntimeException ignore) {
            // Malformed config → no names; StreamService.open will surface the real error.
        }
        return names;
    }

    private MemorySegment requireOpen() {
        MemorySegment svc = service;
        if (svc == null) {
            throw new IllegalStateException("StreamService is closed");
        }
        return svc;
    }

    /** Flush every buffer, stop the export engines, and free the native service. Idempotent. */
    @Override
    public synchronized void close() {
        MemorySegment svc = service;
        if (svc != null) {
            service = null;
            n.shutdown(svc);
        }
    }
}
