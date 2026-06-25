/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.breissinger.ggcommons.metrics.targets;

import com.breissinger.ggcommons.config.MetricConfiguration;
import com.breissinger.ggcommons.config.ConfigurationFactory;
import com.breissinger.ggcommons.metrics.Metric;
import com.breissinger.ggcommons.metrics.MetricBuilder;
import com.breissinger.ggcommons.test.MockConfigurationService;
import com.breissinger.ggcommons.test.MockMessagingService;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import org.junit.jupiter.api.Test;
import software.amazon.awssdk.aws.greengrass.model.QOS;

import java.util.HashMap;
import java.util.List;
import java.util.Map;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Unit tests for the {@link Messaging} metric target using {@link MockMessagingService}
 * to capture published messages without a broker.
 */
class MessagingTest {

    /** Config that selects the messaging target with a caller-supplied topic and destination. */
    private static class MsgConfig extends MockConfigurationService {
        private final MetricConfiguration metricConfig;

        MsgConfig(String topic, String destination, boolean largeFleet) {
            String json = "{\"target\":\"messaging\",\"namespace\":\"ns1\",\"largeFleetWorkaround\":" + largeFleet
                    + ",\"targetConfig\":{\"topic\":\"" + topic + "\",\"destination\":\"" + destination + "\"}}";
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
                .addMeasure("value", "Count", 60)
                .build();
    }

    private static Map<String, Float> values() {
        var v = new HashMap<String, Float>();
        v.put("value", 11.0f);
        return v;
    }

    @Test
    void emitMetricPublishesToIpc() {
        var messaging = new Messaging(new MsgConfig("metrics/topic", "ipc", false));
        var mock = new MockMessagingService();
        messaging.setMessagingService(mock);

        messaging.emitMetric(metric(), values());

        List<MockMessagingService.PublishedMessage> published = mock.getPublishedMessages();
        assertEquals(1, published.size());
        assertEquals("metrics/topic", published.get(0).topic);
        assertNotNull(published.get(0).message);
        // IPC publish path uses no QOS.
        assertNull(published.get(0).qos);
    }

    @Test
    void emitMetricNowPublishesToIotCoreWhenDestinationNotIpc() {
        var messaging = new Messaging(new MsgConfig("metrics/topic", "iotcore", false));
        var mock = new MockMessagingService();
        messaging.setMessagingService(mock);

        messaging.emitMetricNow(metric(), values());

        List<MockMessagingService.PublishedMessage> published = mock.getPublishedMessages();
        assertEquals(1, published.size());
        assertEquals(QOS.AT_LEAST_ONCE, published.get(0).qos);
    }

    @Test
    void largeFleetWorkaroundPublishesTwice() {
        var messaging = new Messaging(new MsgConfig("metrics/topic", "ipc", true));
        var mock = new MockMessagingService();
        messaging.setMessagingService(mock);

        messaging.emitMetricNow(metric(), values());

        assertEquals(2, mock.getPublishedMessages().size());
    }

    @Test
    void onConfigurationChangedRecomputesTopicAndDestination() {
        var messaging = new Messaging(new MsgConfig("metrics/topic", "ipc", false));
        var mock = new MockMessagingService();
        messaging.setMessagingService(mock);

        assertTrue(messaging.onConfigurationChanged());

        // Topic/destination re-resolved from config; publishing still works.
        messaging.emitMetricNow(metric(), values());
        assertEquals(1, mock.getPublishedMessages().size());
        assertEquals("metrics/topic", mock.getPublishedMessages().get(0).topic);
    }

    @Test
    void templateInTopicIsResolved() {
        // MockConfigurationService.resolveTemplate replaces {ComponentName} / {ThingName}.
        var messaging = new Messaging(new MsgConfig("{ThingName}/{ComponentName}/metric", "ipc", false));
        var mock = new MockMessagingService();
        messaging.setMessagingService(mock);

        messaging.emitMetric(metric(), values());

        String topic = mock.getPublishedMessages().get(0).topic;
        assertEquals("test-thing/TestComponent/metric", topic);
    }
}
