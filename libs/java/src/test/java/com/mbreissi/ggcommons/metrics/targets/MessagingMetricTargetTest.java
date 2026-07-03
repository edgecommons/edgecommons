/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.metrics.targets;

import com.mbreissi.ggcommons.config.ConfigurationFactory;
import com.mbreissi.ggcommons.config.MetricConfiguration;
import com.mbreissi.ggcommons.metrics.Metric;
import com.mbreissi.ggcommons.metrics.MetricBuilder;
import com.mbreissi.ggcommons.test.MockConfigurationService;
import com.mbreissi.ggcommons.test.MockMessagingService;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import org.junit.jupiter.api.Test;

import java.util.HashMap;
import java.util.List;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Verifies the {@link Messaging} metric target routes by destination: IoT Core only
 * for {@code iot_core}/{@code iotcore}, otherwise the local/IPC transport (incl. the
 * canonical {@code ipc} and the legacy {@code local}). Both routes carry the UNS metric
 * topic and go through the privileged reserved-publish seam (§4.3).
 */
class MessagingMetricTargetTest {

    private static class MsgConfig extends MockConfigurationService {
        private final MetricConfiguration metricConfig;

        MsgConfig(String destination) {
            var root = new JsonObject();
            String json = "{\"target\":\"messaging\",\"namespace\":\"ns\","
                    + "\"targetConfig\":{\"destination\":\"" + destination + "\"}}";
            root.add("metricEmission", JsonParser.parseString(json).getAsJsonObject());
            this.metricConfig = ConfigurationFactory.createMetricConfiguration(root);
        }

        @Override
        public MetricConfiguration getMetricConfig() {
            return metricConfig;
        }
    }

    private static Metric metric() {
        return MetricBuilder.create("m").withNamespace("ns").addMeasure("v", "Count", 60).build();
    }

    private static void emit(Messaging target) {
        var values = new HashMap<String, Float>();
        values.put("v", 1.0f);
        target.emitMetricNow(metric(), values);
    }

    @Test
    void localDestinationsPublishLocally() {
        for (String dest : new String[]{"ipc", "local"}) {
            Messaging target = new Messaging(new MsgConfig(dest));
            MockMessagingService client = new MockMessagingService();
            target.setMessagingService(client);

            emit(target);

            List<MockMessagingService.PublishedMessage> published = client.getPublishedMessages();
            assertEquals(1, published.size(), "destination " + dest);
            assertNull(published.get(0).qos, "local/IPC publishes carry no QOS (destination " + dest + ")");
            assertTrue(published.get(0).reserved);
            assertEquals("ecv1/test-thing/TestComponent/main/metric/m", published.get(0).topic);
        }
    }

    @Test
    void iotCoreDestinationsPublishToIotCore() {
        for (String dest : new String[]{"iot_core", "iotcore"}) {
            Messaging target = new Messaging(new MsgConfig(dest));
            MockMessagingService client = new MockMessagingService();
            target.setMessagingService(client);

            emit(target);

            List<MockMessagingService.PublishedMessage> published = client.getPublishedMessages();
            assertEquals(1, published.size(), "destination " + dest);
            assertNotNull(published.get(0).qos,
                    "IoT Core publishes carry a QOS (destination " + dest + ")");
            assertTrue(published.get(0).reserved);
            assertEquals("ecv1/test-thing/TestComponent/main/metric/m", published.get(0).topic);
        }
    }
}
