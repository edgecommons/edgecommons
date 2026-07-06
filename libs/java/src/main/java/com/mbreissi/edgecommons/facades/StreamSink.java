/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.facades;

/**
 * The seam {@link DataFacade} composes to route a {@code stream:<name>} channel into the telemetry
 * streaming service (DESIGN-class-facades §4: "the facade <i>composes</i> {@code StreamService}, it
 * does not replace it"). Production wires it to
 * {@code getStreams().stream(name).append(partitionKey, timestampMs, payload)}; it is a functional
 * interface so tests inject a recorder and the facade never depends on the native
 * {@code edgestreamlog} binding.
 *
 * <p>When streaming is not configured ({@code getStreams() == null}), the instance handle passes a
 * {@code null} sink and the facade falls the stream route back to a LOCAL publish (readiness /
 * no-streaming → local) rather than dropping the record.
 */
@FunctionalInterface
public interface StreamSink {

    /**
     * Appends one durable record to a named stream.
     *
     * @param streamName   the configured stream name (the {@code stream:<name>} target)
     * @param partitionKey the routing/ordering key — the signal's stable {@code signal.id}
     * @param timestampMs  the producer timestamp (epoch millis, from the sample's {@code serverTs})
     * @param payload      the serialized envelope bytes (the exact bytes a bus publish would carry)
     */
    void append(String streamName, String partitionKey, long timestampMs, byte[] payload);
}
