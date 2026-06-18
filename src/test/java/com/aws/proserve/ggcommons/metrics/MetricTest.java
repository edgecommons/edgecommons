/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.metrics;

import com.google.gson.JsonArray;
import com.google.gson.JsonElement;
import org.junit.jupiter.api.Test;
import software.amazon.awssdk.services.cloudwatch.model.Dimension;

import java.util.Collection;
import java.util.HashMap;
import java.util.Map;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Unit tests for {@link Metric} serialization helpers that are not covered elsewhere:
 * {@code dimensionsAsJson} (with/without coreName), {@code dimensionsAsCollection}
 * (with/without the large-fleet workaround), and the dimension/measure accessors.
 */
class MetricTest {

    private static Metric metricWithCoreName() {
        var measures = new HashMap<String, Measure>();
        measures.put("count", new Measure("count", "Count", 1));
        var dims = new HashMap<String, String>();
        dims.put("coreName", "thing-1");
        dims.put("component", "com.example.Component");
        // Metric constructor adds "category" = name.
        return new Metric("requests", "NS", measures, dims);
    }

    private static String valueForName(JsonArray arr, String name) {
        for (JsonElement el : arr) {
            if (el.getAsJsonObject().get("name").getAsString().equals(name)) {
                return el.getAsJsonObject().get("value").getAsString();
            }
        }
        return null;
    }

    private static boolean containsName(JsonArray arr, String name) {
        for (JsonElement el : arr) {
            if (el.getAsJsonObject().get("name").getAsString().equals(name)) {
                return true;
            }
        }
        return false;
    }

    @Test
    void dimensionsAsJsonDefaultIncludesCoreName() {
        Metric metric = metricWithCoreName();
        JsonArray json = metric.dimensionsAsJson(); // defaults to includeCoreName=true

        assertTrue(containsName(json, "coreName"));
        assertEquals("thing-1", valueForName(json, "coreName"));
        assertTrue(containsName(json, "component"));
        assertTrue(containsName(json, "category"));
        assertEquals("requests", valueForName(json, "category"));
    }

    @Test
    void dimensionsAsJsonCanExcludeCoreName() {
        Metric metric = metricWithCoreName();
        JsonArray json = metric.dimensionsAsJson(false);

        assertFalse(containsName(json, "coreName"));
        // non-coreName dimensions remain present
        assertTrue(containsName(json, "component"));
        assertTrue(containsName(json, "category"));
    }

    @Test
    void dimensionsAsCollectionWithoutWorkaroundUsesRealCoreName() {
        Metric metric = metricWithCoreName();
        Collection<Dimension> dims = metric.dimensionsAsCollection(); // largeFleetWorkaround=false

        Dimension coreName = dims.stream()
                .filter(d -> d.name().equals("coreName"))
                .findFirst().orElse(null);
        assertNotNull(coreName);
        assertEquals("thing-1", coreName.value());
    }

    @Test
    void dimensionsAsCollectionWithWorkaroundReplacesCoreNameWithAll() {
        Metric metric = metricWithCoreName();
        Collection<Dimension> dims = metric.dimensionsAsCollection(true);

        Dimension coreName = dims.stream()
                .filter(d -> d.name().equals("coreName"))
                .findFirst().orElse(null);
        assertNotNull(coreName);
        assertEquals("ALL", coreName.value());

        // Non-coreName dimensions keep their real value even with the workaround.
        Dimension component = dims.stream()
                .filter(d -> d.name().equals("component"))
                .findFirst().orElse(null);
        assertNotNull(component);
        assertEquals("com.example.Component", component.value());
    }

    @Test
    void getMeasureAndGetDimensionsAccessors() {
        Metric metric = metricWithCoreName();

        assertNotNull(metric.getMeasure("count"));
        assertNull(metric.getMeasure("does-not-exist"));
        assertEquals("requests", metric.getName());
        assertEquals("NS", metric.getNamespace());

        Map<String, String> dimensions = metric.getDimensions();
        assertEquals("thing-1", dimensions.get("coreName"));
        assertEquals("requests", dimensions.get("category"));
    }
}
