/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.breissinger.ggcommons.metrics;

import com.breissinger.ggcommons.config.MetricConfiguration;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.*;
import static org.mockito.Mockito.*;

/**
 * Unit tests for the {@link MetricBuilder#build(MetricEmitter)} overload, which fills
 * the namespace, thing name and component name from the supplied {@link MetricEmitter}
 * when they were not set explicitly. Complements MetricBuilderTest (which covers the
 * no-arg build()/validation paths).
 */
class MetricBuilderEmitterTest {

    private MetricEmitter mockEmitter(String namespace, String thingName, String componentName) {
        MetricEmitter emitter = mock(MetricEmitter.class);
        MetricConfiguration metricConfig = mock(MetricConfiguration.class);
        when(metricConfig.getNamespace()).thenReturn(namespace);
        when(emitter.getMetricConfig()).thenReturn(metricConfig);
        when(emitter.getThingName()).thenReturn(thingName);
        when(emitter.getComponentName()).thenReturn(componentName);
        return emitter;
    }

    @Test
    void buildWithEmitterPopulatesNamespaceThingAndComponent() {
        MetricEmitter emitter = mockEmitter("EmitterNamespace", "emitter-thing", "emitter.Component");

        Metric metric = MetricBuilder.create("latency")
                .addMeasure("ms", "Milliseconds", 1)
                .build(emitter);

        assertEquals("EmitterNamespace", metric.getNamespace());
        assertEquals("emitter-thing", metric.getDimensions().get("coreName"));
        assertEquals("emitter.Component", metric.getDimensions().get("component"));
        assertEquals("latency", metric.getDimensions().get("category"));
    }

    @Test
    void buildWithEmitterDoesNotOverrideExplicitValues() {
        MetricEmitter emitter = mockEmitter("EmitterNamespace", "emitter-thing", "emitter.Component");

        Metric metric = MetricBuilder.create("latency")
                .withNamespace("ExplicitNamespace")
                .withThingName("explicit-thing")
                .withComponentName("explicit.Component")
                .addMeasure("ms", "Milliseconds", 1)
                .build(emitter);

        // Explicit values win; emitter is only a fallback.
        assertEquals("ExplicitNamespace", metric.getNamespace());
        assertEquals("explicit-thing", metric.getDimensions().get("coreName"));
        assertEquals("explicit.Component", metric.getDimensions().get("component"));
        // getMetricConfig() is never consulted because namespace was set.
        verify(emitter, never()).getMetricConfig();
    }

    @Test
    void buildWithEmitterThrowsWhenNoMeasures() {
        MetricEmitter emitter = mockEmitter("EmitterNamespace", "emitter-thing", "emitter.Component");

        MetricBuilder builder = MetricBuilder.create("empty");
        assertThrows(IllegalStateException.class, () -> builder.build(emitter));
    }
}
