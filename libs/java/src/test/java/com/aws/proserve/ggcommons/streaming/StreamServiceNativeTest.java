/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.streaming;

import com.aws.proserve.ggcommons.config.ConfigManager;
import com.aws.proserve.ggcommons.config.MetricConfiguration;
import com.aws.proserve.ggcommons.metrics.MetricEmitter;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.junit.jupiter.api.Assumptions.assumeTrue;
import static org.mockito.ArgumentMatchers.any;
import static org.mockito.ArgumentMatchers.anyMap;
import static org.mockito.ArgumentMatchers.eq;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.timeout;
import static org.mockito.Mockito.verify;
import static org.mockito.Mockito.when;

/**
 * Exercises the native {@code ggstreamlog} binding over the C ABI (Panama). Requires the cdylib;
 * set {@code -Dggstreamlog.library.path=<path to ggstreamlog.dll/.so>} (built with
 * {@code cargo build --features cabi}). Skipped otherwise. Buffer-only — no AWS needed.
 */
class StreamServiceNativeTest {

    private static final int N = 1000;

    @BeforeAll
    static void requireNativeLib() {
        String p = System.getProperty("ggstreamlog.library.path");
        assumeTrue(p != null && !p.isBlank() && Files.exists(Path.of(p)),
                "set -Dggstreamlog.library.path to run the native streaming test");
    }

    private static String config(Path bufferDir) {
        String path = bufferDir.toString().replace('\\', '/');
        return """
                {"streams":[{
                  "name":"telemetry",
                  "sink":{"type":"kinesis","streamName":"x"},
                  "buffer":{"path":"%s","segmentBytes":65536,"maxDiskBytes":1073741824,"onFull":"block"}
                }]}""".formatted(path);
    }

    @Test
    void openAppendFlushAndStats() throws Exception {
        Path dir = Files.createTempDirectory("ggsl-java-it");
        try (StreamService svc = StreamService.open(config(dir));
             StreamHandle h = svc.stream("telemetry")) {

            for (int i = 0; i < N; i++) {
                byte[] payload = ("reading-" + i).getBytes(StandardCharsets.UTF_8);
                h.append("pump-7", 1000L + i, payload);
            }
            h.flush();

            StreamStats s = svc.stats("telemetry");
            assertEquals(N, s.appendedTotal(), "every record appended");
            assertEquals(N, s.nextOffset(), "offsets contiguous 0..N");
            assertEquals(N, s.backlog(), "buffer-only: nothing exported yet");
            assertEquals(0, s.droppedTotal(), "block policy never drops");
            assertTrue(s.diskBytes() > 0, "records are on disk");
        }
    }

    @Test
    void unknownStreamReportsErrorCode() throws Exception {
        Path dir = Files.createTempDirectory("ggsl-java-it2");
        try (StreamService svc = StreamService.open(config(dir))) {
            GgStreamException ex = assertThrows(GgStreamException.class, () -> svc.stats("nope"));
            assertEquals(GgStreamException.ERR_UNKNOWN_STREAM, ex.code());
        }
    }

    @Test
    void badConfigReportsConfigError() {
        GgStreamException ex =
                assertThrows(GgStreamException.class, () -> StreamService.open("{ not valid json"));
        assertEquals(GgStreamException.ERR_CONFIG, ex.code());
    }

    @Test
    void streamNamesParsedFromConfig() throws Exception {
        List<String> names = StreamService.streamNames(config(Files.createTempDirectory("ggsl-names")));
        assertEquals(List.of("telemetry"), names);
    }

    @Test
    void metricsBridgeDefinesAndEmitsPerStream() throws Exception {
        Path dir = Files.createTempDirectory("ggsl-bridge");
        ConfigManager cm = mock(ConfigManager.class);
        MetricConfiguration mc = mock(MetricConfiguration.class);
        when(mc.getNamespace()).thenReturn("ns");
        when(cm.getMetricConfig()).thenReturn(mc);
        when(cm.getThingName()).thenReturn("thing");
        when(cm.getComponentName()).thenReturn("comp");
        MetricEmitter metrics = mock(MetricEmitter.class);

        try (StreamService svc = StreamService.open(config(dir));
             StreamHandle h = svc.stream("telemetry")) {
            for (int i = 0; i < 50; i++) {
                h.append("k", 1000L + i, ("v" + i).getBytes(StandardCharsets.UTF_8));
            }
            h.flush();
            // 1s interval: defineMetric at construction, emitMetric on the first tick.
            try (StreamMetricsBridge bridge =
                         new StreamMetricsBridge(cm, metrics, svc, List.of("telemetry"), 1)) {
                verify(metrics, timeout(2000)).defineMetric(any());
                verify(metrics, timeout(4000).atLeastOnce()).emitMetric(eq("stream:telemetry"), anyMap());
            }
        }
    }
}
