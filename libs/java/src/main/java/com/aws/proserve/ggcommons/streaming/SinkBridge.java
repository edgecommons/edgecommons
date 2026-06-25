/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.streaming;

import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.lang.foreign.Arena;
import java.lang.foreign.FunctionDescriptor;
import java.lang.foreign.MemoryLayout;
import java.lang.foreign.MemorySegment;
import java.lang.invoke.MethodHandle;
import java.lang.invoke.MethodHandles;
import java.lang.invoke.MethodType;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;

import static java.lang.foreign.ValueLayout.ADDRESS;
import static java.lang.foreign.ValueLayout.JAVA_INT;
import static java.lang.foreign.ValueLayout.JAVA_LONG;

/**
 * Bridges a {@code callback}-type ggstreamlog stream to a host {@link SinkFunction}. Registers a
 * single process-wide Panama upcall as the C {@code ggsl_set_sink_callback}; the export engine
 * invokes it per batch on its background thread. The stub reads the borrowed C batch into
 * JVM-owned {@link SinkRecord}s, calls the registered {@link SinkFunction}, and writes the
 * {@link SinkOutcome} back into the core-owned {@code GgslSinkOutcome} out-struct.
 *
 * <p>The native registration must happen <em>before</em> {@link StreamService#open} (the core binds
 * the callback per stream at open time). The {@link SinkFunction} itself may be set/replaced at any
 * time via {@link #setSink}; until one is set, every batch is reported as a retryable failure so the
 * durable buffer holds it (at-least-once) rather than dropping it.
 *
 * <p>Run with {@code --enable-native-access=ALL-UNNAMED} (FFM restricted methods).
 */
final class SinkBridge {

    private static final Logger LOGGER = LogManager.getLogger(SinkBridge.class);

    // ----- C-ABI struct layouts (must match ggstreamlog.h / ffi.rs) -----

    // struct ggsl_sink_record_t { u64 offset; u64 ts_ms; const u8* pk; usize pk_len;
    //                             const u8* payload; usize payload_len; }   (48 bytes, 8-aligned)
    private static final MemoryLayout RECORD_LAYOUT = MemoryLayout.structLayout(
            JAVA_LONG.withName("offset"),
            JAVA_LONG.withName("ts_ms"),
            ADDRESS.withName("pk"),
            JAVA_LONG.withName("pk_len"),
            ADDRESS.withName("payload"),
            JAVA_LONG.withName("payload_len"));
    private static final long REC_OFFSET = 0;
    private static final long REC_TS = 8;
    private static final long REC_PK = 16;
    private static final long REC_PK_LEN = 24;
    private static final long REC_PAYLOAD = 32;
    private static final long REC_PAYLOAD_LEN = 40;
    private static final long REC_SIZE = RECORD_LAYOUT.byteSize(); // 48

    // struct ggsl_sink_outcome_t { c_int status; u64* failed_offsets; usize failed_cap;
    //                              usize failed_count; }  (i32 + pad + 3*8 = 32 bytes)
    private static final long OUT_STATUS = 0;
    private static final long OUT_FAILED_OFFSETS = 8;
    private static final long OUT_FAILED_CAP = 16;
    private static final long OUT_FAILED_COUNT = 24;

    // GGSL_SINK_* status codes (ffi.rs).
    private static final int GGSL_OK = 0;

    /** The single registered host sink. Volatile: written by the metrics layer, read on the engine thread. */
    private static volatile SinkFunction sink;

    private static boolean registered;
    @SuppressWarnings("unused") // kept reachable so the upcall stub is not collected
    private static MemorySegment stub;

    private SinkBridge() {
    }

    /**
     * Register (replace) the host sink function and ensure the native upcall is installed. Call this
     * before opening the stream that drains through it. Idempotent for the native registration; the
     * function itself can be swapped freely.
     */
    static synchronized void setSink(SinkFunction fn) {
        sink = fn;
        if (registered) {
            return;
        }
        GgStreamNative n = GgStreamNative.instance();
        try {
            MethodHandle target = MethodHandles.lookup().findStatic(
                    SinkBridge.class, "onBatch",
                    MethodType.methodType(int.class, MemorySegment.class, MemorySegment.class,
                            long.class, MemorySegment.class));
            stub = GgStreamNative.linker().upcallStub(target,
                    FunctionDescriptor.of(JAVA_INT, ADDRESS, ADDRESS, JAVA_LONG, ADDRESS),
                    Arena.global());
            n.setSinkCallback(stub, MemorySegment.NULL);
            registered = true;
        } catch (Throwable t) {
            throw new IllegalStateException("could not install ggstreamlog sink bridge", t);
        }
    }

    /** Clear the registered sink (subsequent batches become retryable failures → held on disk). */
    static synchronized void clearSink() {
        sink = null;
    }

    /**
     * The C {@code ggsl_sink_cb}: {@code (user_data, records, n, *outcome) -> int}. Invoked on the
     * export engine thread with a borrowed batch. Never throws back across the FFI boundary; on any
     * internal error it leaves the outcome at its default (retryable) so the engine holds the batch.
     */
    @SuppressWarnings("unused") // invoked via the upcall stub
    static int onBatch(MemorySegment userData, MemorySegment records, long n, MemorySegment outcome) {
        try {
            SinkFunction fn = sink;
            List<SinkRecord> batch = readBatch(records, n);
            SinkOutcome result = (fn == null) ? SinkOutcome.retryable() : fn.send(batch);
            writeOutcome(outcome, result);
            return GGSL_OK;
        } catch (Throwable t) {
            // Default outcome status is FAILED_RETRYABLE (set by the core) — leave it; the engine
            // re-delivers. Returning OK keeps the engine on its normal retry/backoff path.
            try {
                LOGGER.error("sink bridge upcall failed; batch will be retried", t);
            } catch (Throwable ignore) {
                // never propagate into native code
            }
            return GGSL_OK;
        }
    }

    /** Copy the borrowed native batch into JVM-owned records (pointers are valid only for this call). */
    private static List<SinkRecord> readBatch(MemorySegment records, long n) {
        // Reinterpret the (zero-length) incoming pointer so we can index into the array.
        MemorySegment arr = records.reinterpret(REC_SIZE * Math.max(n, 0));
        List<SinkRecord> out = new ArrayList<>((int) Math.max(n, 0));
        for (long i = 0; i < n; i++) {
            long base = i * REC_SIZE;
            long offset = arr.get(JAVA_LONG, base + REC_OFFSET);
            long tsMs = arr.get(JAVA_LONG, base + REC_TS);
            MemorySegment pkPtr = arr.get(ADDRESS, base + REC_PK);
            long pkLen = arr.get(JAVA_LONG, base + REC_PK_LEN);
            MemorySegment plPtr = arr.get(ADDRESS, base + REC_PAYLOAD);
            long plLen = arr.get(JAVA_LONG, base + REC_PAYLOAD_LEN);

            String pk = readUtf8(pkPtr, pkLen);
            byte[] payload = readBytes(plPtr, plLen);
            out.add(new SinkRecord(offset, tsMs, pk, payload));
        }
        return out;
    }

    private static String readUtf8(MemorySegment ptr, long len) {
        if (ptr.address() == 0 || len <= 0) {
            return "";
        }
        return new String(readBytes(ptr, len), StandardCharsets.UTF_8);
    }

    private static byte[] readBytes(MemorySegment ptr, long len) {
        if (ptr.address() == 0 || len <= 0) {
            return new byte[0];
        }
        return ptr.reinterpret(len).toArray(java.lang.foreign.ValueLayout.JAVA_BYTE);
    }

    /** Write the {@link SinkOutcome} into the core-owned {@code GgslSinkOutcome} out-struct. */
    private static void writeOutcome(MemorySegment outcome, SinkOutcome result) {
        MemorySegment out = outcome.reinterpret(OUT_FAILED_COUNT + JAVA_LONG.byteSize());
        out.set(JAVA_INT, OUT_STATUS, result.status().code());
        if (result.status() == SinkOutcome.Status.PARTIAL && !result.failedOffsets().isEmpty()) {
            MemorySegment failedPtr = out.get(ADDRESS, OUT_FAILED_OFFSETS);
            long cap = out.get(JAVA_LONG, OUT_FAILED_CAP);
            List<Long> failed = result.failedOffsets();
            long count = Math.min(failed.size(), cap);
            if (failedPtr.address() != 0 && count > 0) {
                MemorySegment buf = failedPtr.reinterpret(cap * JAVA_LONG.byteSize());
                for (long i = 0; i < count; i++) {
                    buf.setAtIndex(JAVA_LONG, i, failed.get((int) i));
                }
            }
            out.set(JAVA_LONG, OUT_FAILED_COUNT, count);
        }
    }
}
