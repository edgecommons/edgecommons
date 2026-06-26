/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.test;

import com.mbreissi.ggcommons.metrics.Metric;
import com.mbreissi.ggcommons.metrics.MetricEmitter;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

/**
 * Mock implementation of IMetricService for testing.
 */
public class MockMetricService extends MetricEmitter {
    private final Map<String, Metric> definedMetrics = new HashMap<>();
    private final List<EmittedMetric> emittedMetrics = new ArrayList<>();
    
    public static class EmittedMetric {
        public final String name;
        public final Map<String, Float> measureValues;
        public final boolean immediate;
        
        public EmittedMetric(String name, Map<String, Float> measureValues, boolean immediate) {
            this.name = name;
            this.measureValues = new HashMap<>(measureValues);
            this.immediate = immediate;
        }
    }
    
    @Override
    public void defineMetric(Metric metric) {
        definedMetrics.put(metric.getName(), metric);
    }
    
    @Override
    public void emitMetric(String name, Map<String, Float> measureValues) {
        emittedMetrics.add(new EmittedMetric(name, measureValues, false));
    }
    
    @Override
    public void emitMetricNow(String name, Map<String, Float> measureValues) {
        emittedMetrics.add(new EmittedMetric(name, measureValues, true));
    }
    
    @Override
    public boolean isMetricDefined(String name) {
        return definedMetrics.containsKey(name);
    }
    
    // Test utility methods
    public List<EmittedMetric> getEmittedMetrics() {
        return new ArrayList<>(emittedMetrics);
    }
    
    public void clearEmittedMetrics() {
        emittedMetrics.clear();
    }
    
    public Map<String, Metric> getDefinedMetrics() {
        return new HashMap<>(definedMetrics);
    }
    
    public void clearDefinedMetrics() {
        definedMetrics.clear();
    }
    
    public void reset() {
        definedMetrics.clear();
        emittedMetrics.clear();
    }
}