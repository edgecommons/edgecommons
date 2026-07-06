/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.streaming;

/**
 * One record handed to a host {@link SinkFunction} by the export engine. The {@code offset} is the
 * record's log offset (use it to report failed offsets in a {@link SinkOutcome.Status#PARTIAL}
 * outcome); {@code partitionKey} is the routing key (the namespace, for the CloudWatch drain) and
 * {@code payload} the opaque record bytes (the compact {@code {namespace, datum}} JSON).
 *
 * <p>Immutable value type. The byte arrays are copies owned by the JVM (the native batch is borrowed
 * only for the duration of the upcall), so a {@code SinkRecord} is safe to retain.
 */
public record SinkRecord(long offset, long timestampMs, String partitionKey, byte[] payload) {
}
