/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.breissinger.ggcommons.streaming;

import com.breissinger.ggcommons.config.ConfigManager;
import com.breissinger.ggcommons.metrics.MetricBuilder;
import com.breissinger.ggcommons.metrics.MetricEmitter;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.TimeUnit;

/**
 * Periodically polls each telemetry stream's {@link StreamStats} and emits them through the
 * component's {@link MetricEmitter}, so streaming metrics land in the same configured target
 * (CloudWatch / messaging / log) as heartbeat and the rest. Mirrors the Rust lib's
 * {@code StreamMetricsBridge}. One metric per stream, named {@code stream:<name>}.
 */
public final class StreamMetricsBridge implements AutoCloseable {

    private static final Logger LOGGER = LogManager.getLogger(StreamMetricsBridge.class);
    private static final long DEFAULT_INTERVAL_SECS = 30;

    private final StreamService streams;
    private final List<String> names;
    private final MetricEmitter metrics;
    private final ScheduledExecutorService scheduler =
            Executors.newSingleThreadScheduledExecutor(r -> {
                Thread t = new Thread(r, "StreamMetrics-scheduler");
                t.setDaemon(true);
                return t;
            });
    private final ScheduledFuture<?> task;

    public StreamMetricsBridge(ConfigManager config, MetricEmitter metrics,
                               StreamService streams, List<String> names) {
        this(config, metrics, streams, names, DEFAULT_INTERVAL_SECS);
    }

    public StreamMetricsBridge(ConfigManager config, MetricEmitter metrics,
                               StreamService streams, List<String> names, long intervalSecs) {
        this.streams = streams;
        this.names = List.copyOf(names);
        this.metrics = metrics;
        int resolution = intervalSecs < 60 ? 1 : 60;
        for (String name : this.names) {
            metrics.defineMetric(MetricBuilder.create(metricName(name))
                    .withConfig(config)
                    .addMeasure("backlog", "Count", resolution)
                    .addMeasure("droppedTotal", "Count", resolution)
                    .addMeasure("exportedTotal", "Count", resolution)
                    .addMeasure("retriesTotal", "Count", resolution)
                    .addMeasure("failedTotal", "Count", resolution)
                    .addMeasure("diskBytes", "Bytes", resolution)
                    .addMeasure("oldestUnackedAgeMs", "Milliseconds", resolution)
                    .build());
        }
        task = scheduler.scheduleAtFixedRate(this::tick, intervalSecs, intervalSecs, TimeUnit.SECONDS);
        LOGGER.info("Stream metrics bridge started for {} stream(s) at {}s interval",
                this.names.size(), intervalSecs);
    }

    private static String metricName(String stream) {
        return "stream:" + stream;
    }

    /** Emit current stats for every stream. Guarded — telemetry-about-telemetry must never crash. */
    private void tick() {
        for (String name : names) {
            try {
                StreamStats s = streams.stats(name);
                Map<String, Float> values = new HashMap<>(8);
                values.put("backlog", (float) s.backlog());
                values.put("droppedTotal", (float) s.droppedTotal());
                values.put("exportedTotal", (float) s.exportedTotal());
                values.put("retriesTotal", (float) s.retriesTotal());
                values.put("failedTotal", (float) s.failedTotal());
                values.put("diskBytes", (float) s.diskBytes());
                values.put("oldestUnackedAgeMs", (float) s.oldestUnackedAgeMs());
                metrics.emitMetric(metricName(name), values);
            } catch (Exception e) {
                LOGGER.debug("failed to emit stats for stream {}: {}", name, e.getMessage());
            }
        }
    }

    @Override
    public void close() {
        task.cancel(false);
        scheduler.shutdownNow();
    }
}
