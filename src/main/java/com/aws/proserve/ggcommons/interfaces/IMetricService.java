/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.interfaces;

import com.aws.proserve.ggcommons.metrics.Metric;
import java.util.Map;

/**
 * Interface for metric emission services.
 * Provides metric definition and emission capabilities.
 */
public interface IMetricService {
    
    /**
     * Defines a new metric for emission.
     * 
     * @param metric The metric definition to register
     */
    void defineMetric(Metric metric);
    
    /**
     * Emits metric values for a defined metric.
     * Values may be batched according to the metric's configuration.
     * 
     * @param name The name of the metric to emit
     * @param measureValues Map of measure names to their values
     */
    void emitMetric(String name, Map<String, Float> measureValues);
    
    /**
     * Immediately emits metric values without buffering.
     * 
     * @param name The name of the metric to emit
     * @param measureValues Map of measure names to their values
     */
    void emitMetricNow(String name, Map<String, Float> measureValues);
    
    /**
     * Checks if a metric is defined.
     * 
     * @param name The metric name to check
     * @return true if the metric is defined, false otherwise
     */
    boolean isMetricDefined(String name);
}