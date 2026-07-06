/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.metrics.targets;

import com.mbreissi.edgecommons.config.MetricConfiguration;
import com.mbreissi.edgecommons.config.ConfigurationFactory;
import com.mbreissi.edgecommons.metrics.Metric;
import com.mbreissi.edgecommons.metrics.MetricBuilder;
import com.mbreissi.edgecommons.streaming.StreamService;
import com.mbreissi.edgecommons.test.MockConfigurationService;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;
import software.amazon.awssdk.services.cloudwatch.CloudWatchClient;
import software.amazon.awssdk.services.cloudwatch.model.PutMetricDataRequest;
import software.amazon.awssdk.services.cloudwatch.model.PutMetricDataResponse;

import java.nio.file.Files;
import java.nio.file.Path;
import java.util.HashMap;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.jupiter.api.Assertions.*;
import static org.junit.jupiter.api.Assumptions.assumeTrue;
import static org.mockito.ArgumentMatchers.any;
import static org.mockito.Mockito.*;

/**
 * Integration test for the durable CloudWatch buffer (edgestreamlog Callback stream + Panama upcall +
 * the {@link CloudWatchDrain}). Uses a MOCKED {@link CloudWatchClient} (no real AWS). Requires the
 * native {@code edgestreamlog} cdylib; skipped if it cannot be loaded.
 *
 * <p>The headline scenario is disconnect fault-injection: sever CloudWatch → the buffer accumulates
 * on disk (memory flat, component keeps running) → reconnect → the engine drains.
 */
class CloudWatchDurableTest {

    @BeforeAll
    static void requireNativeLib() {
        assumeTrue(StreamService.nativeAvailable(), "edgestreamlog native library not available");
    }

    private CloudWatch cloudWatch;

    /** A MockConfigurationService whose metric config + buffer path we control. */
    private static final class CwConfig extends MockConfigurationService {
        private final MetricConfiguration metricConfig;

        CwConfig(Path bufferDir) {
            String path = bufferDir.toString().replace('\\', '/');
            String metricJson = "{\"target\":\"cloudwatch\",\"namespace\":\"ns1\","
                    + "\"targetConfig\":{\"intervalSecs\":1,\"buffer\":{"
                    + "\"type\":\"durable\",\"path\":\"" + path + "\","
                    + "\"maxDiskBytes\":1073741824,\"onFull\":\"dropOldest\",\"fsync\":\"perBatch\"}}}";
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
        return MetricBuilder.create(name).withNamespace(namespace).addMeasure("value", "Count", 60).build();
    }

    @AfterEach
    void tearDown() {
        if (cloudWatch != null) {
            cloudWatch.close();
            cloudWatch = null;
        }
    }

    @Test
    void durableBufferOpensAndDrainsOnConnect() throws Exception {
        Path dir = Files.createTempDirectory("esl-cw-drain");
        CwConfig config = new CwConfig(dir);
        assumeTrue(config.getMetricConfig().getBufferConfig().isDurable());

        CloudWatchClient client = mock(CloudWatchClient.class);
        when(client.putMetricData(any(PutMetricDataRequest.class)))
                .thenReturn(PutMetricDataResponse.builder().build());
        cloudWatch = new CloudWatch(config, client);

        var values = new HashMap<String, Float>();
        values.put("value", 1.0f);
        for (int i = 0; i < 20; i++) {
            cloudWatch.emitMetric(metric("m" + i, "ns1"), values);
        }
        cloudWatch.flush();

        // The background engine drains the disk buffer to PutMetricData.
        verify(client, timeout(8000).atLeastOnce()).putMetricData(any(PutMetricDataRequest.class));
    }

    @Test
    void severThenReconnectStoreAndForward() throws Exception {
        Path dir = Files.createTempDirectory("esl-cw-sever");
        CwConfig config = new CwConfig(dir);
        assumeTrue(config.getMetricConfig().getBufferConfig().isDurable());

        AtomicBoolean connected = new AtomicBoolean(false);
        AtomicInteger acceptedRequests = new AtomicInteger();
        CloudWatchClient client = mock(CloudWatchClient.class);
        when(client.putMetricData(any(PutMetricDataRequest.class))).thenAnswer(inv -> {
            if (!connected.get()) {
                throw new RuntimeException("network down (severed)");
            }
            acceptedRequests.incrementAndGet();
            return PutMetricDataResponse.builder().build();
        });

        cloudWatch = new CloudWatch(config, client);

        // --- Phase 1: SEVERED. Emit a burst; nothing is accepted; the backlog persists on disk. ---
        var values = new HashMap<String, Float>();
        values.put("value", 1.0f);
        for (int i = 0; i < 50; i++) {
            cloudWatch.emitMetric(metric("m" + i, "ns1"), values);
        }
        cloudWatch.flush();

        // The engine attempts (and fails) PutMetricData while severed; backlog stays on disk.
        verify(client, timeout(8000).atLeastOnce()).putMetricData(any(PutMetricDataRequest.class));
        // Records persisted to disk (memory stays flat — they are not held in a JVM queue).
        assertTrue(Files.exists(dir), "buffer directory exists");
        long diskBytes = directorySize(dir);
        assertTrue(diskBytes > 0, "severed backlog is on disk (" + diskBytes + " bytes)");
        assertEquals(0, acceptedRequests.get(), "nothing accepted while severed");

        // --- Phase 2: RECONNECT. The engine retries and the backlog drains. ---
        connected.set(true);
        // Wait for the export engine's retry/backoff loop to push the backlog through.
        long deadline = System.currentTimeMillis() + 15000;
        while (acceptedRequests.get() == 0 && System.currentTimeMillis() < deadline) {
            Thread.sleep(100);
        }
        assertTrue(acceptedRequests.get() > 0, "backlog drained after reconnect");
    }

    @Test
    void emitMetricNowAlsoBuffersInDurableMode() throws Exception {
        Path dir = Files.createTempDirectory("esl-cw-now");
        CwConfig config = new CwConfig(dir);
        assumeTrue(config.getMetricConfig().getBufferConfig().isDurable());

        CloudWatchClient client = mock(CloudWatchClient.class);
        when(client.putMetricData(any(PutMetricDataRequest.class)))
                .thenReturn(PutMetricDataResponse.builder().build());
        cloudWatch = new CloudWatch(config, client);

        var values = new HashMap<String, Float>();
        values.put("value", 99.0f);
        cloudWatch.emitMetricNow(metric("now", "ns1"), values);
        cloudWatch.flush();

        // Always-buffer: emitMetricNow goes through the durable buffer and the engine drains it.
        verify(client, timeout(8000).atLeastOnce()).putMetricData(any(PutMetricDataRequest.class));
        assertEquals(0, cloudWatch.getDroppedStale());
    }

    @Test
    void onConfigurationChangedIsNoOpInDurableMode() throws Exception {
        Path dir = Files.createTempDirectory("esl-cw-cfg");
        CwConfig config = new CwConfig(dir);
        assumeTrue(config.getMetricConfig().getBufferConfig().isDurable());

        CloudWatchClient client = mock(CloudWatchClient.class);
        when(client.putMetricData(any(PutMetricDataRequest.class)))
                .thenReturn(PutMetricDataResponse.builder().build());
        cloudWatch = new CloudWatch(config, client);

        assertTrue(cloudWatch.onConfigurationChanged());
    }

    /** A config pointing the durable buffer at a path that cannot be a directory (a regular file). */
    private static final class BadPathConfig extends MockConfigurationService {
        private final MetricConfiguration metricConfig;

        BadPathConfig(Path file) {
            String path = file.toString().replace('\\', '/');
            String metricJson = "{\"target\":\"cloudwatch\",\"namespace\":\"ns1\","
                    + "\"targetConfig\":{\"intervalSecs\":1,\"buffer\":{"
                    + "\"type\":\"durable\",\"path\":\"" + path + "\","
                    + "\"maxDiskBytes\":1073741824}}}";
            var root = new JsonObject();
            root.add("metricEmission", JsonParser.parseString(metricJson).getAsJsonObject());
            this.metricConfig = ConfigurationFactory.createMetricConfiguration(root);
        }

        @Override
        public MetricConfiguration getMetricConfig() {
            return metricConfig;
        }
    }

    @Test
    void durableInitFailureFallsBackToMemory() throws Exception {
        // Point the buffer "directory" at an existing regular file → the disk buffer cannot open →
        // the target must fall back to in-memory batching and keep working.
        Path file = Files.createTempFile("esl-cw-badpath", ".notadir");
        BadPathConfig config = new BadPathConfig(file);
        assumeTrue(config.getMetricConfig().getBufferConfig().isDurable());

        CloudWatchClient client = mock(CloudWatchClient.class);
        when(client.putMetricData(any(PutMetricDataRequest.class)))
                .thenReturn(PutMetricDataResponse.builder().build());
        cloudWatch = new CloudWatch(config, client);

        // Memory fallback: emitMetricNow sends immediately (durable would buffer instead).
        var values = new HashMap<String, Float>();
        values.put("value", 1.0f);
        cloudWatch.emitMetricNow(metric("m", "ns1"), values);

        verify(client, timeout(4000).atLeastOnce()).putMetricData(any(PutMetricDataRequest.class));
        // Memory mode has no drain → dropped-stale counter is zero.
        assertEquals(0, cloudWatch.getDroppedStale());
    }

    private static long directorySize(Path dir) throws Exception {
        try (var stream = Files.walk(dir)) {
            return stream.filter(Files::isRegularFile).mapToLong(p -> {
                try {
                    return Files.size(p);
                } catch (Exception e) {
                    return 0L;
                }
            }).sum();
        }
    }
}
