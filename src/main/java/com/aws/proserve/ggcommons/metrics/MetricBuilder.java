package com.aws.proserve.ggcommons.metrics;

import java.util.HashMap;
import java.util.Map;

/**
 * Builder for creating Metric instances with fluent API.
 */
public class MetricBuilder {
    private String name;
    private String namespace;
    private Map<String, Measure> measures = new HashMap<>();
    private Map<String, String> dimensions = new HashMap<>();
    
    private MetricBuilder() {}
    
    public static MetricBuilder create(String name) {
        MetricBuilder builder = new MetricBuilder();
        builder.name = name;
        return builder;
    }
    
    public MetricBuilder withNamespace(String namespace) {
        this.namespace = namespace;
        return this;
    }
    
    public MetricBuilder addMeasure(String name, String unit, int precision) {
        if (name == null || name.trim().isEmpty()) {
            throw new IllegalArgumentException("Measure name cannot be null or empty");
        }
        if (measures.containsKey(name)) {
            throw new IllegalArgumentException("Measure with name '" + name + "' already exists");
        }
        this.measures.put(name, new Measure(name, unit, precision));
        return this;
    }
    
    public MetricBuilder addMeasure(Measure measure) {
        if (measure == null) {
            throw new IllegalArgumentException("Measure cannot be null");
        }
        if (measures.containsKey(measure.getName())) {
            throw new IllegalArgumentException("Measure with name '" + measure.getName() + "' already exists");
        }
        this.measures.put(measure.getName(), measure);
        return this;
    }
    
    public MetricBuilder addDimension(String key, String value) {
        if (key == null || key.trim().isEmpty()) {
            throw new IllegalArgumentException("Dimension key cannot be null or empty");
        }
        if (dimensions.size() >= 10) {
            throw new IllegalArgumentException("Maximum of 10 dimensions allowed per metric");
        }
        this.dimensions.put(key, value);
        return this;
    }
    
    public Metric build(MetricEmitter metricEmitter) {
        if (namespace == null) {
            namespace = metricEmitter.getMetricConfig().getNamespace();
        }
        return new Metric(name, namespace, measures, dimensions);
    }
    
    public Metric build() {
        if (namespace == null) {
            throw new IllegalStateException("Namespace must be set or MetricEmitter instance must be provided");
        }
        if (measures.isEmpty()) {
            throw new IllegalStateException("At least one measure must be defined for a metric");
        }
        return new Metric(name, namespace, measures, dimensions);
    }
}