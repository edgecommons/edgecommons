/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.streaming;

import java.lang.foreign.Arena;
import java.lang.foreign.MemorySegment;

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

    /** The config document this service was opened with. */
    public String configJson() {
        return configJson;
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
