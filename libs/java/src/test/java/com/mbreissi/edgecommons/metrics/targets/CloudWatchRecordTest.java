/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.metrics.targets;

import org.junit.jupiter.api.Test;
import software.amazon.awssdk.services.cloudwatch.model.Dimension;
import software.amazon.awssdk.services.cloudwatch.model.MetricDatum;
import software.amazon.awssdk.services.cloudwatch.model.StandardUnit;

import java.nio.charset.StandardCharsets;
import java.time.Instant;
import java.util.List;

import static org.junit.jupiter.api.Assertions.*;

/** Round-trip + edge-case tests for the durable CloudWatch record format. */
class CloudWatchRecordTest {

    @Test
    void roundTripsAllFields() {
        Instant ts = Instant.ofEpochMilli(1_700_000_000_000L);
        MetricDatum datum = MetricDatum.builder()
                .metricName("Latency")
                .value(42.5)
                .unit(StandardUnit.MILLISECONDS)
                .storageResolution(1)
                .timestamp(ts)
                .dimensions(
                        Dimension.builder().name("category").value("Latency").build(),
                        Dimension.builder().name("coreName").value("device-1").build())
                .build();

        byte[] bytes = CloudWatchRecord.serialize("ns-A", datum);
        CloudWatchRecord.Parsed parsed = CloudWatchRecord.deserialize(bytes);

        assertEquals("ns-A", parsed.namespace());
        assertEquals(ts, parsed.timestamp());
        MetricDatum d = parsed.datum();
        assertEquals("Latency", d.metricName());
        assertEquals(42.5, d.value());
        assertEquals(StandardUnit.MILLISECONDS, d.unit());
        assertEquals(1, d.storageResolution());
        assertEquals(2, d.dimensions().size());
        assertEquals("category", d.dimensions().get(0).name());
        assertEquals("device-1", d.dimensions().get(1).value());
    }

    @Test
    void roundTripsWithoutDimensionsOrTimestamp() {
        MetricDatum datum = MetricDatum.builder().metricName("Count").value(1.0).build();
        CloudWatchRecord.Parsed parsed = CloudWatchRecord.deserialize(CloudWatchRecord.serialize("ns", datum));
        assertEquals("ns", parsed.namespace());
        assertEquals("Count", parsed.datum().metricName());
        assertEquals(Instant.EPOCH, parsed.timestamp());
        assertFalse(parsed.datum().hasDimensions());
    }

    @Test
    void partitionKeyIsNamespaceInJson() {
        MetricDatum datum = MetricDatum.builder().metricName("X").value(1.0).build();
        String json = new String(CloudWatchRecord.serialize("my-ns", datum), StandardCharsets.UTF_8);
        assertTrue(json.contains("\"namespace\":\"my-ns\""));
        assertTrue(json.contains("\"datum\""));
    }

    @Test
    void deserializeRejectsNonObject() {
        byte[] bytes = "[1,2,3]".getBytes(StandardCharsets.UTF_8);
        assertThrows(IllegalArgumentException.class, () -> CloudWatchRecord.deserialize(bytes));
    }

    @Test
    void deserializeRejectsMissingFields() {
        byte[] bytes = "{\"namespace\":\"x\"}".getBytes(StandardCharsets.UTF_8);
        assertThrows(IllegalArgumentException.class, () -> CloudWatchRecord.deserialize(bytes));
    }

    @Test
    void deserializeRejectsGarbage() {
        byte[] bytes = "not json at all".getBytes(StandardCharsets.UTF_8);
        assertThrows(RuntimeException.class, () -> CloudWatchRecord.deserialize(bytes));
    }

    @Test
    void serializeToJsonOmitsNullUnit() {
        MetricDatum datum = MetricDatum.builder().metricName("NoUnit").value(3.0).build();
        var obj = CloudWatchRecord.serializeToJson("ns", datum);
        assertFalse(obj.getAsJsonObject("datum").has("unit"));
        // dimensions array is always present (possibly empty)
        assertTrue(obj.getAsJsonObject("datum").getAsJsonArray("dimensions").isEmpty());
        // sanity: deserializes back
        CloudWatchRecord.Parsed p = CloudWatchRecord.deserialize(
                obj.toString().getBytes(StandardCharsets.UTF_8));
        assertEquals(List.of(), p.datum().dimensions());
    }
}
