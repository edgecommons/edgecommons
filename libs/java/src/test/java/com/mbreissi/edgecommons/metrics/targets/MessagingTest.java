/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.metrics.targets;

import com.mbreissi.edgecommons.config.MetricConfiguration;
import com.mbreissi.edgecommons.config.ConfigurationFactory;
import com.mbreissi.edgecommons.metrics.Metric;
import com.mbreissi.edgecommons.metrics.MetricBuilder;
import com.mbreissi.edgecommons.test.MockConfigurationService;
import com.mbreissi.edgecommons.test.MockMessagingService;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import org.junit.jupiter.api.Test;

import java.util.HashMap;
import java.util.List;
import java.util.Map;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Unit tests for the {@link Messaging} metric target on its UNS topic scheme (UNS-CANONICAL-DESIGN
 * §4.3): each metric publishes to {@code ecv1/{device}/{component}/metric/{metricName}} (the
 * name sanitized as a channel token) through the privileged {@code ReservedPublisher} seam, with
 * {@code targetConfig.destination} still selecting local vs northbound (D-U9). Uses
 * {@link MockMessagingService} to capture publishes without a broker.
 */
class MessagingTest {

    /** The default mock identity's UNS metric topic prefix (device=test-thing, component=TestComponent). */
    private static final String METRIC_TOPIC_PREFIX = "ecv1/test-thing/TestComponent/metric/";

    /** Config that selects the messaging target with a caller-supplied destination. */
    private static class MsgConfig extends MockConfigurationService {
        private final MetricConfiguration metricConfig;

        MsgConfig(String destination, boolean largeFleet) {
            String json = "{\"target\":\"messaging\",\"namespace\":\"ns1\",\"largeFleetWorkaround\":" + largeFleet
                    + ",\"targetConfig\":{\"destination\":\"" + destination + "\"}}";
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
        return metric("m1");
    }

    private static Metric metric(String name) {
        return MetricBuilder.create(name)
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
    void emitMetricPublishesToTheUnsMetricTopic() {
        var messaging = new Messaging(new MsgConfig("ipc", false));
        var mock = new MockMessagingService();
        messaging.setMessagingService(mock);

        messaging.emitMetric(metric(), values());

        List<MockMessagingService.PublishedMessage> published = mock.getPublishedMessages();
        assertEquals(1, published.size());
        assertEquals(METRIC_TOPIC_PREFIX + "m1", published.get(0).topic,
                "the metric topic is ecv1/{device}/{component}/metric/{metricName}");
        assertNotNull(published.get(0).message);
        assertTrue(published.get(0).reserved,
                "metric publishes must go through the privileged ReservedPublisher seam");
        // IPC publish path uses no QOS.
        assertNull(published.get(0).qos);
    }

    @Test
    void emitMetricNowPublishesToIotCoreApiWhenDestinationNorthbound() {
        var messaging = new Messaging(new MsgConfig("northbound", false));
        var mock = new MockMessagingService();
        messaging.setMessagingService(mock);

        messaging.emitMetricNow(metric(), values());

        List<MockMessagingService.PublishedMessage> published = mock.getPublishedMessages();
        assertEquals(1, published.size());
        assertEquals(METRIC_TOPIC_PREFIX + "m1", published.get(0).topic);
        assertNotNull(published.get(0).qos, "northbound destination publishes via publishNorthbound");
        assertTrue(published.get(0).reserved);
    }

    @Test
    void metricNameIsSanitizedAsAChannelToken() {
        var messaging = new Messaging(new MsgConfig("ipc", false));
        var mock = new MockMessagingService();
        messaging.setMessagingService(mock);

        // '/', '+', '#' are the sanitizer's blacklist -> '_'; spaces and dots survive (§2.2).
        messaging.emitMetric(metric("api/errors+5xx #hot v1.2"), values());

        assertEquals(METRIC_TOPIC_PREFIX + "api_errors_5xx _hot v1.2",
                mock.getPublishedMessages().get(0).topic);
    }

    @Test
    void largeFleetWorkaroundPublishesTwice() {
        var messaging = new Messaging(new MsgConfig("ipc", true));
        var mock = new MockMessagingService();
        messaging.setMessagingService(mock);

        messaging.emitMetricNow(metric(), values());

        assertEquals(2, mock.getPublishedMessages().size());
        assertEquals(METRIC_TOPIC_PREFIX + "m1", mock.getPublishedMessages().get(1).topic);
    }

    @Test
    void onConfigurationChangedRecomputesDestination() {
        var messaging = new Messaging(new MsgConfig("ipc", false));
        var mock = new MockMessagingService();
        messaging.setMessagingService(mock);

        assertTrue(messaging.onConfigurationChanged());

        // Destination re-resolved from config; publishing still works on the UNS topic.
        messaging.emitMetricNow(metric(), values());
        assertEquals(1, mock.getPublishedMessages().size());
        assertEquals(METRIC_TOPIC_PREFIX + "m1", mock.getPublishedMessages().get(0).topic);
    }

    @Test
    void missingIdentityDropsTheMetric() {
        var config = new MsgConfig("ipc", false);
        config.setComponentIdentity(null); // the test/subclass bring-up case
        var messaging = new Messaging(config);
        var mock = new MockMessagingService();
        messaging.setMessagingService(mock);

        messaging.emitMetricNow(metric(), values());

        assertTrue(mock.getPublishedMessages().isEmpty(),
                "no resolved identity -> no UNS metric topic -> the metric is dropped");
    }
}
