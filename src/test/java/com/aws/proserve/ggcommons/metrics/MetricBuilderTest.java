/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.metrics;

import org.junit.jupiter.api.Test;
import static org.junit.jupiter.api.Assertions.*;

/**
 * Unit tests for MetricBuilder class.
 * Tests the builder pattern methods for creating Metric instances.
 */
class MetricBuilderTest {

    @Test
    void testBuilderWithNamespace() {
        MetricBuilder builder = MetricBuilder.create("test-metric")
                .withNamespace("TestNamespace");
        
        assertNotNull(builder);
    }
    
    @Test
    void testBuilderAddMeasureWithParameters() {
        MetricBuilder builder = MetricBuilder.create("test-metric")
                .addMeasure("response_time", "Milliseconds", 1)
                .addMeasure("error_count", "Count", 60);
        
        assertNotNull(builder);
    }
    
    @Test
    void testBuilderAddMeasureWithObject() {
        Measure measure1 = new Measure("latency", "Milliseconds", 1);
        Measure measure2 = new Measure("throughput", "Count/Second", 1);
        
        MetricBuilder builder = MetricBuilder.create("test-metric")
                .addMeasure(measure1)
                .addMeasure(measure2);
        
        assertNotNull(builder);
    }
    
    @Test
    void testBuilderAddDimension() {
        MetricBuilder builder = MetricBuilder.create("test-metric")
                .addDimension("Environment", "Production")
                .addDimension("Service", "TestService")
                .addDimension("Region", "us-west-2");
        
        assertNotNull(builder);
    }
    
    @Test
    void testDimensionLimit() {
        MetricBuilder builder = MetricBuilder.create("test-metric")
                .withNamespace("TestNamespace");
        
        // Add 10 dimensions (should work)
        for (int i = 0; i < 10; i++) {
            builder.addDimension("dim" + i, "value" + i);
        }
        
        // Adding 11th dimension should fail
        assertThrows(IllegalArgumentException.class, () -> 
            builder.addDimension("dim10", "value10"));
    }
    
    @Test
    void testBuilderChaining() {
        Measure measure = new Measure("count", "Count", 1);
        
        MetricBuilder builder = MetricBuilder.create("test-metric")
                .withNamespace("TestNamespace")
                .addMeasure("value", "Count", 1)
                .addMeasure(measure)
                .addDimension("env", "test");
        
        assertNotNull(builder);
    }
    
    @Test
    void testBuilderValidation() {
        MetricBuilder builder = MetricBuilder.create("test-metric")
                .withNamespace("TestNamespace");
        
        // Test duplicate measure names
        builder.addMeasure("count", "Count", 1);
        assertThrows(IllegalArgumentException.class, () -> 
            builder.addMeasure("count", "Count", 1));
        
        // Test null measure
        assertThrows(IllegalArgumentException.class, () -> 
            builder.addMeasure(null));
        
        // Test empty measure name
        assertThrows(IllegalArgumentException.class, () -> 
            builder.addMeasure("", "Count", 1));
        
        // Test null dimension key
        assertThrows(IllegalArgumentException.class, () -> 
            builder.addDimension(null, "value"));
    }
    
    @Test
    void testBuildValidation() {
        // Test build without measures
        MetricBuilder builder = MetricBuilder.create("test-metric")
                .withNamespace("TestNamespace");
        
        assertThrows(IllegalStateException.class, () -> builder.build());
        
        // Test build without namespace
        MetricBuilder builder2 = MetricBuilder.create("test-metric")
                .addMeasure("count", "Count", 1);
        
        assertThrows(IllegalStateException.class, () -> builder2.build());
    }

    @Test
    void testStandardDimensionsInjected() {
        Metric metric = MetricBuilder.create("requests")
                .withNamespace("TestNamespace")
                .withThingName("thing-1")
                .withComponentName("com.example.Component")
                .addMeasure("count", "Count", 1)
                .build();

        // category (= metric name), coreName (= thing name), component (= component name)
        assertEquals("requests", metric.getDimensions().get("category"));
        assertEquals("thing-1", metric.getDimensions().get("coreName"));
        assertEquals("com.example.Component", metric.getDimensions().get("component"));
    }
}