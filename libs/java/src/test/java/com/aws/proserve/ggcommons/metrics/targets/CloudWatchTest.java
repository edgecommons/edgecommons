/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.metrics.targets;

import com.aws.proserve.ggcommons.config.MetricConfiguration;
import com.aws.proserve.ggcommons.config.ConfigurationFactory;
import com.aws.proserve.ggcommons.metrics.Metric;
import com.aws.proserve.ggcommons.metrics.MetricBuilder;
import com.aws.proserve.ggcommons.streaming.StreamService;
import com.aws.proserve.ggcommons.test.MockConfigurationService;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.Test;
import org.mockito.MockedStatic;
import software.amazon.awssdk.services.cloudwatch.CloudWatchClient;
import software.amazon.awssdk.services.cloudwatch.model.CloudWatchException;
import software.amazon.awssdk.services.cloudwatch.model.PutMetricDataRequest;

import java.util.HashMap;
import java.util.Map;

import static org.junit.jupiter.api.Assertions.*;
import static org.mockito.ArgumentMatchers.any;
import static org.mockito.Mockito.*;

/**
 * Unit tests for the {@link CloudWatch} metric target. A mocked {@link CloudWatchClient}
 * is injected via the package-private constructor so no real AWS calls are made.
 */
class CloudWatchTest {

    private CloudWatch cloudWatch;

    /**
     * MockConfigurationService that returns a caller-supplied metric configuration so we
     * can exercise the CloudWatch target (interval, large-fleet workaround) deterministically.
     */
    private static class CwConfig extends MockConfigurationService {
        private final MetricConfiguration metricConfig;

        CwConfig(String metricJson) {
            var root = new JsonObject();
            root.add("metricEmission", JsonParser.parseString(metricJson).getAsJsonObject());
            this.metricConfig = ConfigurationFactory.createMetricConfiguration(root);
        }

        @Override
        public MetricConfiguration getMetricConfig() {
            return metricConfig;
        }
    }

    private static Metric metric(String name, String namespace) {
        return MetricBuilder.create(name)
                .withNamespace(namespace)
                .addMeasure("value", "Count", 60)
                .build();
    }

    @AfterEach
    void tearDown() {
        if (cloudWatch != null) {
            cloudWatch.close();
            cloudWatch = null;
        }
    }

    // --- Scheduling integration tests (ScheduledExecutorService-based flush) ---

    @Test
    void periodicFlushSendsQueuedMetrics() {
        var config = new CwConfig("{\"target\":\"cloudwatch\",\"namespace\":\"ns1\",\"targetConfig\":{\"intervalSecs\":1,\"buffer\":{\"type\":\"memory\"}}}");
        CloudWatchClient client = mock(CloudWatchClient.class);
        cloudWatch = new CloudWatch(config, client);

        var values = new HashMap<String, Float>();
        values.put("value", 1.0f);
        cloudWatch.emitMetric(metric("m1", "ns1"), values);

        // The 1s scheduled flush must send the queued metric within a few seconds.
        verify(client, timeout(4000).atLeastOnce()).putMetricData(any(PutMetricDataRequest.class));
    }

    @Test
    void durableWithAbsentNativeCoreFailsFast() {
        // A durable cloudwatch config, but the ggstreamlog native core is unavailable -> construction
        // must FAIL FAST rather than silently fall back to in-memory: durable is the default and is
        // bundled by design, so a missing core is a deployment error (silent degradation would lose
        // metrics across a disconnect). A bad PATH with the core present still falls back (see
        // CloudWatchDurableTest#durableInitFailureFallsBackToMemory).
        var config = new CwConfig("{\"target\":\"cloudwatch\",\"namespace\":\"ns1\","
                + "\"targetConfig\":{\"buffer\":{\"type\":\"durable\",\"path\":\"build/ggsl-absent\"}}}");
        CloudWatchClient client = mock(CloudWatchClient.class);
        try (MockedStatic<StreamService> ss = mockStatic(StreamService.class)) {
            ss.when(StreamService::nativeAvailable).thenReturn(false);
            IllegalStateException ex = assertThrows(IllegalStateException.class,
                    () -> new CloudWatch(config, client));
            assertTrue(ex.getMessage().contains("native core"),
                    "error should name the absent native core: " + ex.getMessage());
        }
    }

    @Test
    void configurationChangeReschedulesFlush() {
        var config = new CwConfig("{\"target\":\"cloudwatch\",\"namespace\":\"ns1\",\"targetConfig\":{\"intervalSecs\":1,\"buffer\":{\"type\":\"memory\"}}}");
        CloudWatchClient client = mock(CloudWatchClient.class);
        cloudWatch = new CloudWatch(config, client);

        // Reschedule on the same executor; flushing must keep working afterward.
        assertTrue(cloudWatch.onConfigurationChanged());

        var values = new HashMap<String, Float>();
        values.put("value", 2.0f);
        cloudWatch.emitMetric(metric("m2", "ns1"), values);

        verify(client, timeout(4000).atLeastOnce()).putMetricData(any(PutMetricDataRequest.class));
    }

    @Test
    void emitMetricNowSendsToCloudWatchImmediately() {
        var config = new CwConfig("{\"target\":\"cloudwatch\",\"namespace\":\"ns1\",\"targetConfig\":{\"intervalSecs\":3600,\"buffer\":{\"type\":\"memory\"}}}");
        CloudWatchClient client = mock(CloudWatchClient.class);
        cloudWatch = new CloudWatch(config, client);

        var values = new HashMap<String, Float>();
        values.put("value", 42.0f);
        cloudWatch.emitMetricNow(metric("m1", "ns1"), values);

        verify(client, times(1)).putMetricData(any(PutMetricDataRequest.class));
    }

    @Test
    void emitMetricNowSwallowsCloudWatchException() {
        var config = new CwConfig("{\"target\":\"cloudwatch\",\"namespace\":\"ns1\",\"targetConfig\":{\"intervalSecs\":3600,\"buffer\":{\"type\":\"memory\"}}}");
        CloudWatchClient client = mock(CloudWatchClient.class);
        when(client.putMetricData(any(PutMetricDataRequest.class)))
                .thenThrow(CloudWatchException.builder().message("boom").build());
        cloudWatch = new CloudWatch(config, client);

        var values = new HashMap<String, Float>();
        values.put("value", 1.0f);
        // Must not propagate the exception.
        assertDoesNotThrow(() -> cloudWatch.emitMetricNow(metric("m1", "ns1"), values));
        verify(client, times(1)).putMetricData(any(PutMetricDataRequest.class));
    }

    @Test
    void emitMetricBuffersUntilFlush() {
        var config = new CwConfig("{\"target\":\"cloudwatch\",\"namespace\":\"ns1\",\"targetConfig\":{\"intervalSecs\":3600,\"buffer\":{\"type\":\"memory\"}}}");
        CloudWatchClient client = mock(CloudWatchClient.class);
        cloudWatch = new CloudWatch(config, client);

        var values = new HashMap<String, Float>();
        values.put("value", 5.0f);
        cloudWatch.emitMetric(metric("m1", "ns1"), values);
        cloudWatch.emitMetric(metric("m2", "ns1"), values);

        // Buffered metrics flushed in a single batch (one namespace, well under 1000 datums).
        cloudWatch.flush();
        verify(client, atLeast(1)).putMetricData(any(PutMetricDataRequest.class));
    }

    @Test
    void flushChunksWhenExceeding1000Datums() {
        var config = new CwConfig("{\"target\":\"cloudwatch\",\"namespace\":\"ns1\",\"targetConfig\":{\"intervalSecs\":3600,\"buffer\":{\"type\":\"memory\"}}}");
        CloudWatchClient client = mock(CloudWatchClient.class);
        cloudWatch = new CloudWatch(config, client);

        // Build a single metric carrying 600 measures; one measure value -> one datum.
        MetricBuilder builder = MetricBuilder.create("bulk").withNamespace("ns1");
        var values = new HashMap<String, Float>();
        for (int i = 0; i < 600; i++) {
            builder.addMeasure("v" + i, "Count", 60);
            values.put("v" + i, (float) i);
        }
        Metric bulk = builder.build();

        // Enqueue twice -> 1200 datums in the same namespace -> must chunk into >1 request.
        cloudWatch.emitMetric(bulk, values);
        cloudWatch.emitMetric(bulk, values);

        cloudWatch.flush();
        // 1200 datums / 1000 per request -> 2 putMetricData calls.
        verify(client, times(2)).putMetricData(any(PutMetricDataRequest.class));
    }

    @Test
    void flushIsolatesFailuresPerNamespace() {
        var config = new CwConfig("{\"target\":\"cloudwatch\",\"namespace\":\"ns1\",\"targetConfig\":{\"intervalSecs\":3600,\"buffer\":{\"type\":\"memory\"}}}");
        CloudWatchClient client = mock(CloudWatchClient.class);
        when(client.putMetricData(any(PutMetricDataRequest.class)))
                .thenThrow(new RuntimeException("network down"));
        cloudWatch = new CloudWatch(config, client);

        var values = new HashMap<String, Float>();
        values.put("value", 9.0f);
        cloudWatch.emitMetric(metric("m1", "nsA"), values);
        cloudWatch.emitMetric(metric("m2", "nsB"), values);

        // Both namespaces attempted; failure in one must not stop the other.
        assertDoesNotThrow(() -> cloudWatch.flush());
        verify(client, times(2)).putMetricData(any(PutMetricDataRequest.class));
    }

    @Test
    void largeFleetWorkaroundDoublesDatums() {
        var config = new CwConfig("{\"target\":\"cloudwatch\",\"namespace\":\"ns1\",\"largeFleetWorkaround\":true,\"targetConfig\":{\"intervalSecs\":3600,\"buffer\":{\"type\":\"memory\"}}}");
        CloudWatchClient client = mock(CloudWatchClient.class);
        cloudWatch = new CloudWatch(config, client);

        var values = new HashMap<String, Float>();
        values.put("value", 7.0f);
        assertTrue(config.getMetricConfig().getLargeFleetWorkaround());
        cloudWatch.emitMetricNow(metric("m1", "ns1"), values);

        verify(client, times(1)).putMetricData(any(PutMetricDataRequest.class));
    }

    @Test
    void onConfigurationChangedReinitializesTimer() {
        var config = new CwConfig("{\"target\":\"cloudwatch\",\"namespace\":\"ns1\",\"targetConfig\":{\"intervalSecs\":3600,\"buffer\":{\"type\":\"memory\"}}}");
        CloudWatchClient client = mock(CloudWatchClient.class);
        cloudWatch = new CloudWatch(config, client);

        assertTrue(cloudWatch.onConfigurationChanged());
    }

    @Test
    void closeCancelsTimerAndClosesClient() {
        var config = new CwConfig("{\"target\":\"cloudwatch\",\"namespace\":\"ns1\",\"targetConfig\":{\"intervalSecs\":3600,\"buffer\":{\"type\":\"memory\"}}}");
        CloudWatchClient client = mock(CloudWatchClient.class);
        var cw = new CloudWatch(config, client);

        cw.close();
        verify(client, times(1)).close();
        // Guard against double-close in tearDown.
        cloudWatch = null;
    }

    @Test
    void closeSwallowsClientCloseException() {
        var config = new CwConfig("{\"target\":\"cloudwatch\",\"namespace\":\"ns1\",\"targetConfig\":{\"intervalSecs\":3600,\"buffer\":{\"type\":\"memory\"}}}");
        CloudWatchClient client = mock(CloudWatchClient.class);
        doThrow(new RuntimeException("close failed")).when(client).close();
        var cw = new CloudWatch(config, client);

        assertDoesNotThrow(cw::close);
        cloudWatch = null;
    }
}
