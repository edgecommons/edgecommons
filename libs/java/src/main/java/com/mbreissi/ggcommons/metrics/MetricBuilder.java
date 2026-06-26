package com.mbreissi.ggcommons.metrics;

import com.mbreissi.ggcommons.config.ConfigManager;

import java.util.HashMap;
import java.util.Map;

/**
 * Builder for creating Metric instances with a fluent API.
 *
 * <p>On build the standard dimensions {@code coreName} (thing name) and {@code component}
 * (component name) are injected when known; the {@code category} dimension (= metric name)
 * is added by the {@link Metric} constructor. This matches the Python and Rust libraries.
 */
public class MetricBuilder {
    private String name;
    private String namespace;
    private String thingName;
    private String componentName;
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

    /**
     * Sets the thing name, which becomes the {@code coreName} dimension.
     */
    public MetricBuilder withThingName(String thingName) {
        this.thingName = thingName;
        return this;
    }

    /**
     * Sets the component name, which becomes the {@code component} dimension.
     */
    public MetricBuilder withComponentName(String componentName) {
        this.componentName = componentName;
        return this;
    }

    /**
     * Populates the thing name, component name and (if not already set) namespace from configuration.
     */
    public MetricBuilder withConfig(ConfigManager configManager) {
        this.thingName = configManager.getThingName();
        this.componentName = configManager.getComponentName();
        if (this.namespace == null) {
            this.namespace = configManager.getMetricConfig().getNamespace();
        }
        return this;
    }

    public MetricBuilder addMeasure(String name, String unit, int precision) {
        if (name == null || name.isBlank()) {
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
        if (measures.containsKey(measure.name())) {
            throw new IllegalArgumentException("Measure with name '" + measure.name() + "' already exists");
        }
        this.measures.put(measure.name(), measure);
        return this;
    }

    public MetricBuilder addDimension(String key, String value) {
        if (key == null || key.isBlank()) {
            throw new IllegalArgumentException("Dimension key cannot be null or empty");
        }
        if (!dimensions.containsKey(key) && dimensions.size() >= 10) {
            throw new IllegalArgumentException("Maximum of 10 dimensions allowed per metric");
        }
        this.dimensions.put(key, value);
        return this;
    }

    public Metric build(MetricEmitter metricEmitter) {
        if (namespace == null) {
            namespace = metricEmitter.getMetricConfig().getNamespace();
        }
        if (thingName == null) {
            thingName = metricEmitter.getThingName();
        }
        if (componentName == null) {
            componentName = metricEmitter.getComponentName();
        }
        if (measures.isEmpty()) {
            throw new IllegalStateException("At least one measure must be defined for a metric");
        }
        return assemble();
    }

    public Metric build() {
        if (namespace == null) {
            throw new IllegalStateException("Namespace must be set or MetricEmitter instance must be provided");
        }
        if (measures.isEmpty()) {
            throw new IllegalStateException("At least one measure must be defined for a metric");
        }
        return assemble();
    }

    /**
     * Assembles the Metric, injecting the standard {@code coreName} (thing name) and
     * {@code component} (component name) dimensions when known.
     */
    @SuppressWarnings("deprecation") // MetricBuilder is the sanctioned path to the Metric ctor.
    private Metric assemble() {
        if (thingName != null) {
            dimensions.put("coreName", thingName);
        }
        if (componentName != null) {
            dimensions.put("component", componentName);
        }
        return new Metric(name, namespace, measures, dimensions);
    }
}
