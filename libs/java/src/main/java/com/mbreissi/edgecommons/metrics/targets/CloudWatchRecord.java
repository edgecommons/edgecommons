/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.metrics.targets;

import com.google.gson.JsonArray;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import software.amazon.awssdk.services.cloudwatch.model.Dimension;
import software.amazon.awssdk.services.cloudwatch.model.MetricDatum;
import software.amazon.awssdk.services.cloudwatch.model.StandardUnit;

import java.nio.charset.StandardCharsets;
import java.time.Instant;
import java.util.Map;

/**
 * The on-wire record format for a durable CloudWatch buffer entry: a compact JSON
 * {@code {"namespace": "...", "datum": {...}}} object. The partition key is the {@code namespace}
 * (so the drain can group by it), and the {@code datum} is a self-describing serialization of a
 * single CloudWatch {@link MetricDatum} (name, value, unit, storageResolution, timestamp millis,
 * dimensions). Round-trips losslessly through {@link #serialize}/{@link #deserialize}.
 *
 * <p>Kept separate from {@link EmfHelper} on purpose: the {@code cloudwatch} target builds typed SDK
 * {@code MetricDatum} and calls {@code PutMetricData} (it is not EMF), so the durable record mirrors
 * the {@code MetricDatum} fields rather than the EMF envelope.
 */
final class CloudWatchRecord {

    private CloudWatchRecord() {
    }

    /** Serialize one {@code (namespace, datum)} pair to the compact JSON record (UTF-8 bytes). */
    static byte[] serialize(String namespace, MetricDatum datum) {
        return serializeToJson(namespace, datum).toString().getBytes(StandardCharsets.UTF_8);
    }

    /** Serialize one {@code (namespace, datum)} pair to its JSON object (for tests / composition). */
    static JsonObject serializeToJson(String namespace, MetricDatum datum) {
        JsonObject root = new JsonObject();
        root.addProperty("namespace", namespace);

        JsonObject d = new JsonObject();
        d.addProperty("name", datum.metricName());
        if (datum.value() != null) {
            d.addProperty("value", datum.value());
        }
        if (datum.unit() != null) {
            d.addProperty("unit", datum.unitAsString());
        }
        if (datum.storageResolution() != null) {
            d.addProperty("storageResolution", datum.storageResolution());
        }
        if (datum.timestamp() != null) {
            d.addProperty("ts", datum.timestamp().toEpochMilli());
        }
        JsonArray dims = new JsonArray();
        if (datum.hasDimensions()) {
            for (Dimension dim : datum.dimensions()) {
                JsonObject dj = new JsonObject();
                dj.addProperty("name", dim.name());
                dj.addProperty("value", dim.value());
                dims.add(dj);
            }
        }
        d.add("dimensions", dims);
        root.add("datum", d);
        return root;
    }

    /**
     * Deserialize a record's payload bytes back into a {@link Parsed} (namespace + rebuilt
     * {@code MetricDatum}). Throws {@link IllegalArgumentException} on malformed input.
     */
    static Parsed deserialize(byte[] payload) {
        String json = new String(payload, StandardCharsets.UTF_8);
        JsonElement rootEl = JsonParser.parseString(json);
        if (!rootEl.isJsonObject()) {
            throw new IllegalArgumentException("CloudWatch record is not a JSON object");
        }
        JsonObject root = rootEl.getAsJsonObject();
        if (!root.has("namespace") || !root.has("datum")) {
            throw new IllegalArgumentException("CloudWatch record missing namespace/datum");
        }
        String namespace = root.get("namespace").getAsString();
        JsonObject d = root.getAsJsonObject("datum");

        MetricDatum.Builder b = MetricDatum.builder();
        if (d.has("name")) {
            b.metricName(d.get("name").getAsString());
        }
        if (d.has("value")) {
            b.value(d.get("value").getAsDouble());
        }
        if (d.has("unit")) {
            b.unit(StandardUnit.fromValue(d.get("unit").getAsString()));
        }
        Instant ts = Instant.EPOCH;
        if (d.has("ts")) {
            ts = Instant.ofEpochMilli(d.get("ts").getAsLong());
            b.timestamp(ts);
        }
        if (d.has("storageResolution")) {
            b.storageResolution(d.get("storageResolution").getAsInt());
        }
        if (d.has("dimensions") && d.get("dimensions").isJsonArray()) {
            JsonArray dims = d.getAsJsonArray("dimensions");
            var list = new java.util.ArrayList<Dimension>(dims.size());
            for (JsonElement de : dims) {
                JsonObject dj = de.getAsJsonObject();
                list.add(Dimension.builder()
                        .name(dj.get("name").getAsString())
                        .value(dj.get("value").getAsString())
                        .build());
            }
            if (!list.isEmpty()) {
                b.dimensions(list);
            }
        }
        return new Parsed(namespace, b.build(), ts);
    }

    /** A deserialized record: its namespace, the rebuilt datum, and the datum's timestamp. */
    record Parsed(String namespace, MetricDatum datum, Instant timestamp) {
    }
}
