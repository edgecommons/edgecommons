/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.metrics;

import com.aws.proserve.ggcommons.interfaces.IMetricService;
import java.util.Map;

/**
 * Service implementation that wraps MetricEmitter to provide the IMetricService interface.
 * This allows for dependency injection while maintaining backward compatibility.
 */
public class MetricService implements IMetricService {
    
    private final MetricEmitter metricEmitter;
    
    public MetricService(MetricEmitter metricEmitter) {
        this.metricEmitter = metricEmitter;
    }
    
    @Override
    public void defineMetric(Metric metric) {
        metricEmitter.defineMetric(metric);
    }
    
    @Override
    public void emitMetric(String name, Map<String, Float> measureValues) {
        metricEmitter.emitMetric(name, measureValues);
    }
    
    @Override
    public void emitMetricNow(String name, Map<String, Float> measureValues) {
        metricEmitter.emitMetricNow(name, measureValues);
    }
    
    @Override
    public boolean isMetricDefined(String name) {
        // This would require adding this method to MetricEmitter
        // For now, we'll implement a basic check
        try {
            metricEmitter.emitMetric(name, Map.of());
            return true;
        } catch (Exception e) {
            return false;
        }
    }
}