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
    void testBuilderChaining() {
        Measure measure = new Measure("count", "Count", 1);
        
        MetricBuilder builder = MetricBuilder.create("test-metric")
                .withNamespace("TestNamespace")
                .addMeasure("value", "Count", 1)
                .addMeasure(measure)
                .addDimension("env", "test");
        
        assertNotNull(builder);
    }
}