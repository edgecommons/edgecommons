/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.streaming;

import java.lang.foreign.Arena;
import java.lang.foreign.MemorySegment;
import java.nio.charset.StandardCharsets;

import static java.lang.foreign.ValueLayout.JAVA_BYTE;
import static java.lang.foreign.ValueLayout.ADDRESS;

/**
 * A producer handle to one telemetry stream. {@link #append} persists a record durably (per the
 * stream's fsync policy) and the background engine exports it. Thread-safe — call from many threads.
 *
 * <p>{@link AutoCloseable}: {@link #close()} releases the handle. It is independent of the owning
 * {@link StreamService} and remains valid for append/flush even after the service is closed (export
 * stops, but the durable buffer stays usable).
 */
public final class StreamHandle implements AutoCloseable {

    private final GgStreamNative n;
    private volatile MemorySegment stream; // null after close
    private final String name;

    StreamHandle(GgStreamNative n, MemorySegment stream, String name) {
        this.n = n;
        this.stream = stream;
        this.name = name;
    }

    /** The stream name. */
    public String name() {
        return name;
    }

    /**
     * Append one record. Returns once durable per the stream's fsync policy (the producer blocks per
     * the {@code onFull} backpressure policy).
     *
     * @param partitionKey routing/ordering key (UTF-8; ≤ 65535 bytes)
     * @param timestampMs  producer timestamp (epoch millis)
     * @param payload      opaque record bytes
     * @throws GgStreamException on a buffer/IO/sink error (e.g. {@code ERR_FULL} under rejectNew)
     */
    public void append(String partitionKey, long timestampMs, byte[] payload) {
        MemorySegment s = requireOpen();
        byte[] pk = partitionKey.getBytes(StandardCharsets.UTF_8);
        if (pk.length > 0xFFFF) {
            throw new IllegalArgumentException("partitionKey exceeds 65535 bytes");
        }
        try (Arena a = Arena.ofConfined()) {
            MemorySegment pkSeg = bytes(a, pk);
            MemorySegment plSeg = bytes(a, payload);
            MemorySegment err = a.allocate(ADDRESS);
            int rc = n.append(s, pkSeg, (short) pk.length, timestampMs, plSeg,
                    payload == null ? 0 : payload.length, MemorySegment.NULL, err);
            if (rc != GgStreamException.OK) {
                throw new GgStreamException(rc, n.takeError(err));
            }
        }
    }

    /** Force this stream's buffer durably to disk (does not wait for export to the sink). */
    public void flush() {
        MemorySegment s = requireOpen();
        try (Arena a = Arena.ofConfined()) {
            MemorySegment err = a.allocate(ADDRESS);
            int rc = n.flush(s, err);
            if (rc != GgStreamException.OK) {
                throw new GgStreamException(rc, n.takeError(err));
            }
        }
    }

    private static MemorySegment bytes(Arena a, byte[] b) {
        if (b == null || b.length == 0) {
            return MemorySegment.NULL;
        }
        MemorySegment seg = a.allocate(b.length);
        MemorySegment.copy(b, 0, seg, JAVA_BYTE, 0, b.length);
        return seg;
    }

    private MemorySegment requireOpen() {
        MemorySegment s = stream;
        if (s == null) {
            throw new IllegalStateException("StreamHandle is closed");
        }
        return s;
    }

    /** Release this handle. Idempotent. */
    @Override
    public synchronized void close() {
        MemorySegment s = stream;
        if (s != null) {
            stream = null;
            n.streamFree(s);
        }
    }
}
