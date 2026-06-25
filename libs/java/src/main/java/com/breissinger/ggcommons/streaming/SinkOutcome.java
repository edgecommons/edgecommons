/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.breissinger.ggcommons.streaming;

import java.util.List;

/**
 * The result a host {@link SinkFunction} reports for one batch. Maps onto the core's
 * {@code SendOutcome}: the export engine commits the buffer checkpoint past the batch only on
 * {@link Status#ALL_ACKED}; a {@link Status#PARTIAL} re-delivers just {@link #failedOffsets()};
 * a {@link Status#FAILED_RETRYABLE} re-delivers the whole batch (at-least-once); a
 * {@link Status#FAILED_PERMANENT} signals the host already dropped/logged it (the engine still
 * re-delivers it on the next loop, but the host must not re-send).
 */
public record SinkOutcome(Status status, List<Long> failedOffsets) {

    /** Outcome kinds, mirroring the C {@code GGSL_SINK_*} status codes. */
    public enum Status {
        /** Every record in the batch was stored (status code 0). */
        ALL_ACKED(0),
        /** Only {@link SinkOutcome#failedOffsets()} were not stored; retry just those (code 1). */
        PARTIAL(1),
        /** The whole batch failed but may succeed later (disconnected/throttled/5xx) (code 2). */
        FAILED_RETRYABLE(2),
        /** The whole batch failed permanently; the host has dropped/logged it (code 3). */
        FAILED_PERMANENT(3);

        private final int code;

        Status(int code) {
            this.code = code;
        }

        /** The C-ABI {@code GGSL_SINK_*} status code. */
        public int code() {
            return code;
        }
    }

    /** Everything in the batch was stored. */
    public static SinkOutcome allAcked() {
        return new SinkOutcome(Status.ALL_ACKED, List.of());
    }

    /** Only these offsets were not stored; the engine retries just them. */
    public static SinkOutcome partial(List<Long> failedOffsets) {
        return new SinkOutcome(Status.PARTIAL, List.copyOf(failedOffsets));
    }

    /** The whole batch failed transiently; the engine holds and retries it. */
    public static SinkOutcome retryable() {
        return new SinkOutcome(Status.FAILED_RETRYABLE, List.of());
    }

    /** The whole batch failed permanently; the host has already dropped/logged it. */
    public static SinkOutcome permanent() {
        return new SinkOutcome(Status.FAILED_PERMANENT, List.of());
    }
}
