/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.breissinger.ggcommons.config;

import com.google.gson.JsonObject;

/**
 * The {@code metricEmission.targetConfig.buffer} block for the {@code cloudwatch} target — the
 * durable store-and-forward buffer settings. When {@link #isDurable()}, metrics are appended to an
 * embedded ggstreamlog disk buffer and drained to {@code PutMetricData} via a host-callback sink
 * (surviving lengthy disconnects with flat memory and a disk-bounded backlog); otherwise today's
 * in-memory batching is used.
 *
 * <p>Defaults (when the block is present but a field is omitted) match the canonical schema: type
 * {@code durable}, {@code maxDiskBytes} ~128 MiB, {@code onFull=dropOldest}, {@code fsync=perBatch}.
 * When the block is entirely absent the cloudwatch target still defaults to {@code durable}.
 */
public final class BufferConfiguration {

    /** ~128 MiB default on-disk backlog cap. */
    public static final long DEFAULT_MAX_DISK_BYTES = 134_217_728L;
    private static final String DEFAULT_TYPE = "durable";
    private static final String DEFAULT_ON_FULL = "dropOldest";
    private static final String DEFAULT_FSYNC = "perBatch";
    private static final String DEFAULT_PATH = "/var/lib/ggcommons/metrics/{ComponentName}/cw";

    private final String type;
    private final String path;
    private final long maxDiskBytes;
    private final String onFull;
    private final String fsync;

    private BufferConfiguration(String type, String path, long maxDiskBytes, String onFull, String fsync) {
        this.type = type;
        this.path = path;
        this.maxDiskBytes = maxDiskBytes;
        this.onFull = onFull;
        this.fsync = fsync;
    }

    /** An explicitly in-memory buffer (the default for non-cloudwatch targets). */
    public static BufferConfiguration memory() {
        return new BufferConfiguration("memory", null, DEFAULT_MAX_DISK_BYTES, DEFAULT_ON_FULL, DEFAULT_FSYNC);
    }

    /**
     * Parse a {@code buffer} JSON block. A {@code null} block (absent in config) yields the durable
     * default for the cloudwatch target. Unknown/missing fields fall back to the schema defaults.
     */
    public static BufferConfiguration fromJson(JsonObject buffer) {
        if (buffer == null) {
            return new BufferConfiguration(DEFAULT_TYPE, DEFAULT_PATH, DEFAULT_MAX_DISK_BYTES,
                    DEFAULT_ON_FULL, DEFAULT_FSYNC);
        }
        String type = buffer.has("type") ? buffer.get("type").getAsString() : DEFAULT_TYPE;
        String path = buffer.has("path") ? buffer.get("path").getAsString() : DEFAULT_PATH;
        long maxDisk = buffer.has("maxDiskBytes")
                ? buffer.get("maxDiskBytes").getAsBigDecimal().longValue()
                : DEFAULT_MAX_DISK_BYTES;
        if (maxDisk <= 0) {
            maxDisk = DEFAULT_MAX_DISK_BYTES;
        }
        String onFull = buffer.has("onFull") ? buffer.get("onFull").getAsString() : DEFAULT_ON_FULL;
        String fsync = buffer.has("fsync") ? buffer.get("fsync").getAsString() : DEFAULT_FSYNC;
        return new BufferConfiguration(type, path, maxDisk, onFull, fsync);
    }

    /** {@code true} if the durable disk buffer should be used; {@code false} for in-memory batching. */
    public boolean isDurable() {
        return "durable".equalsIgnoreCase(type);
    }

    public String getType() {
        return type;
    }

    /** Buffer directory template ({@code {ComponentName}}/{@code {ThingName}} — resolve before use). */
    public String getPath() {
        return path;
    }

    public long getMaxDiskBytes() {
        return maxDiskBytes;
    }

    public String getOnFull() {
        return onFull;
    }

    public String getFsync() {
        return fsync;
    }
}
