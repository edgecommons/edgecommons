/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.breissinger.ggcommons.streaming;

/**
 * A point-in-time snapshot of one stream's buffer + export progress. Mirrors the native
 * {@code ggsl_stats_t} (10 unsigned 64-bit counters).
 */
public record StreamStats(
        long appendedTotal,
        long exportedTotal,
        long droppedTotal,
        long retriesTotal,
        long failedTotal,
        long backlog,
        long diskBytes,
        long ackedOffset,
        long nextOffset,
        long oldestUnackedAgeMs) {
}
