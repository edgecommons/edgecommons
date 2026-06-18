/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.metrics.targets;

import com.aws.proserve.ggcommons.config.ConfigurationFactory;
import com.aws.proserve.ggcommons.config.MetricConfiguration;
import com.aws.proserve.ggcommons.messaging.Message;
import com.aws.proserve.ggcommons.messaging.MessagingClient;
import com.aws.proserve.ggcommons.metrics.Metric;
import com.aws.proserve.ggcommons.metrics.MetricBuilder;
import com.aws.proserve.ggcommons.test.MockConfigurationService;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import org.junit.jupiter.api.Test;
import software.amazon.awssdk.aws.greengrass.model.QOS;

import java.util.HashMap;

import static org.mockito.ArgumentMatchers.any;
import static org.mockito.ArgumentMatchers.anyString;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.never;
import static org.mockito.Mockito.times;
import static org.mockito.Mockito.verify;

/**
 * Verifies the {@link Messaging} metric target routes by destination: IoT Core only
 * for {@code iot_core}/{@code iotcore}, otherwise the local/IPC transport (incl. the
 * canonical {@code ipc} and the legacy {@code local}).
 */
class MessagingMetricTargetTest {

    private static class MsgConfig extends MockConfigurationService {
        private final MetricConfiguration metricConfig;

        MsgConfig(String destination) {
            var root = new JsonObject();
            String json = "{\"target\":\"messaging\",\"namespace\":\"ns\","
                    + "\"targetConfig\":{\"topic\":\"m/t\",\"destination\":\"" + destination + "\"}}";
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
            MessagingClient client = mock(MessagingClient.class);
            target.setMessagingService(client);

            emit(target);

            verify(client, times(1)).publish(anyString(), any(Message.class));
            verify(client, never()).publishToIotCore(anyString(), any(Message.class), any(QOS.class));
        }
    }

    @Test
    void iotCoreDestinationsPublishToIotCore() {
        for (String dest : new String[]{"iot_core", "iotcore"}) {
            Messaging target = new Messaging(new MsgConfig(dest));
            MessagingClient client = mock(MessagingClient.class);
            target.setMessagingService(client);

            emit(target);

            verify(client, times(1)).publishToIotCore(anyString(), any(Message.class), any(QOS.class));
            verify(client, never()).publish(anyString(), any(Message.class));
        }
    }
}
