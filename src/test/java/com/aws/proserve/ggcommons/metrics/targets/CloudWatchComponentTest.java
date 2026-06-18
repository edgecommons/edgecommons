/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.metrics.targets;

import com.aws.proserve.ggcommons.config.MetricConfiguration;
import com.aws.proserve.ggcommons.config.ConfigurationFactory;
import com.aws.proserve.ggcommons.metrics.Metric;
import com.aws.proserve.ggcommons.metrics.MetricBuilder;
import com.aws.proserve.ggcommons.test.MockConfigurationService;
import com.aws.proserve.ggcommons.test.MockMessagingService;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import org.junit.jupiter.api.Test;

import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Unit tests for the {@link CloudWatchComponent} metric target. Publishes raw EMF-like
 * payloads to a topic via {@link MockMessagingService}.
 */
class CloudWatchComponentTest {

    /** Config that selects the cloudwatchcomponent target with a caller-supplied topic. */
    private static class CwcConfig extends MockConfigurationService {
        private final MetricConfiguration metricConfig;

        CwcConfig(String topic) {
            String json = "{\"target\":\"cloudwatchcomponent\",\"namespace\":\"ns1\",\"targetConfig\":{\"topic\":\"" + topic + "\"}}";
            var root = new JsonObject();
            root.add("metricEmission", JsonParser.parseString(json).getAsJsonObject());
            this.metricConfig = ConfigurationFactory.createMetricConfiguration(root);
        }

        @Override
        public MetricConfiguration getMetricConfig() {
            return metricConfig;
        }
    }

    private static Metric metric() {
        return MetricBuilder.create("m1")
                .withNamespace("ns1")
                .addMeasure("cpu", "Percent", 60)
                .build();
    }

    @Test
    void emitMetricPublishesRawPerMeasure() {
        var target = new CloudWatchComponent(new CwcConfig("cloudwatch/metric/put"));
        var mock = new MockMessagingService();
        target.setMessagingService(mock);

        var values = new LinkedHashMap<String, Float>();
        values.put("cpu", 55.5f);
        target.emitMetric(metric(), values);

        List<MockMessagingService.PublishedMessage> published = mock.getPublishedMessages();
        assertEquals(1, published.size());
        assertEquals("cloudwatch/metric/put", published.get(0).topic);
        assertNotNull(published.get(0).rawPayload);

        // Verify the raw payload structure built by the target.
        JsonObject request = published.get(0).rawPayload.getAsJsonObject("request");
        assertEquals("ns1", request.get("namespace").getAsString());
        JsonObject metricData = request.getAsJsonObject("metricData");
        assertEquals("cpu", metricData.get("metricName").getAsString());
        assertEquals("Percent", metricData.get("unit").getAsString());
        assertEquals(55.5f, metricData.get("value").getAsFloat());
        assertTrue(metricData.has("dimensions"));
        assertTrue(metricData.has("timestamp"));
    }

    @Test
    void emitMetricNowPublishesOnePerMeasure() {
        var target = new CloudWatchComponent(new CwcConfig("cloudwatch/metric/put"));
        var mock = new MockMessagingService();
        target.setMessagingService(mock);

        Metric twoMeasures = MetricBuilder.create("m2")
                .withNamespace("ns1")
                .addMeasure("cpu", "Percent", 60)
                .addMeasure("mem", "Megabytes", 60)
                .build();

        var values = new LinkedHashMap<String, Float>();
        values.put("cpu", 10.0f);
        values.put("mem", 20.0f);
        target.emitMetricNow(twoMeasures, values);

        // One raw publish per measure value.
        assertEquals(2, mock.getPublishedMessages().size());
    }

    @Test
    void onConfigurationChangedReturnsFalseAndReresolvesTopic() {
        var target = new CloudWatchComponent(new CwcConfig("{ThingName}/cw"));
        var mock = new MockMessagingService();
        target.setMessagingService(mock);

        // CloudWatchComponent.onConfigurationChanged is documented to return false.
        assertFalse(target.onConfigurationChanged());

        var values = new LinkedHashMap<String, Float>();
        values.put("cpu", 1.0f);
        target.emitMetricNow(metric(), values);
        assertEquals("test-thing/cw", mock.getPublishedMessages().get(0).topic);
    }
}
