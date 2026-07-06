/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.metrics.targets;

import com.mbreissi.edgecommons.config.BufferConfiguration;
import com.mbreissi.edgecommons.config.ConfigManager;
import com.mbreissi.edgecommons.metrics.Metric;
import com.mbreissi.edgecommons.streaming.StreamHandle;
import com.mbreissi.edgecommons.streaming.StreamService;
import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import software.amazon.awssdk.services.cloudwatch.CloudWatchClient;
import software.amazon.awssdk.services.cloudwatch.model.CloudWatchException;
import software.amazon.awssdk.services.cloudwatch.model.Dimension;
import software.amazon.awssdk.services.cloudwatch.model.MetricDatum;
import software.amazon.awssdk.services.cloudwatch.model.PutMetricDataRequest;

import java.time.Instant;
import java.util.*;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ConcurrentLinkedQueue;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.TimeUnit;

public final class CloudWatch extends MetricTarget
{

    /** The single callback stream name for the durable CloudWatch buffer. */
    static final String DURABLE_STREAM = "metrics-cw";

    private final CloudWatchClient cwClient;

    private final ConcurrentHashMap<String, ConcurrentLinkedQueue<PendingMetric>> pendingMetrics = new ConcurrentHashMap<>();

    private final ScheduledExecutorService scheduler =
            Executors.newSingleThreadScheduledExecutor(runnable -> {
                Thread thread = new Thread(runnable, "CloudWatch-emit-scheduler");
                thread.setDaemon(true);
                return thread;
            });
    private ScheduledFuture<?> emitTask;

    // ----- durable store-and-forward buffer (edgestreamlog Callback stream) -----
    private final boolean durable;
    private final CloudWatchDrain drain;     // null in memory mode
    private StreamService streamService;     // null in memory mode (or if durable init failed)
    private StreamHandle streamHandle;       // null in memory mode (or if durable init failed)

    public CloudWatch(ConfigManager configManager)
    {
        this(configManager, CloudWatchClient.builder().build());
    }

    /** Package-private constructor allowing a CloudWatch client to be injected for testing. */
    CloudWatch(ConfigManager configManager, CloudWatchClient cwClient)
    {
        super(configManager);
        this.cwClient = cwClient;

        BufferConfiguration buffer = metricConfig.getBufferConfig();
        boolean wantDurable = buffer != null && buffer.isDurable();
        boolean durableUp = false;
        CloudWatchDrain d = null;
        if (wantDurable)
        {
            if (!StreamService.nativeAvailable())
            {
                // Fail fast on an ABSENT native core. Durable is the default and is bundled by
                // design, so a missing core is a deployment error — silently degrading would lose
                // metrics across a cloud disconnect. Opt out explicitly with buffer.type=memory.
                throw new IllegalStateException(
                        "Durable CloudWatch buffer (the default) requires the edgestreamlog native "
                        + "core, which is not bundled/loadable for this platform. Bundle it (see "
                        + "docs/NATIVE_CORE_DELIVERY.md) or set "
                        + "metricEmission.targetConfig.cloudwatch.buffer.type=memory.");
            }
            // The drain owns the namespace grouping / stale-drop / chunk / outcome logic; the sink
            // forwards each PutMetricData chunk through the injected client. A core that is present
            // but whose buffer cannot open (e.g. a bad path) still degrades to in-memory below.
            d = new CloudWatchDrain(this::putChunk);
            durableUp = startDurable(buffer, d);
        }
        this.durable = durableUp;
        this.drain = durableUp ? d : null;

        if (!this.durable)
        {
            // In-memory path (explicit type=memory, or durable init failed → safe fallback).
            scheduleEmit();
        }
    }

    /**
     * Open the durable buffer: register the drain as the host sink, then open a single Callback
     * stream backed by a disk buffer at the (template-resolved) configured path. Returns false (and
     * the target falls back to in-memory batching) if the native streaming core is unavailable.
     */
    private boolean startDurable(BufferConfiguration buffer, CloudWatchDrain d)
    {
        try
        {
            // Resolve {ComponentName}/{ThingName} in the buffer path BEFORE opening — this also
            // closes the known Java streaming template-resolution gap for this path.
            String resolvedPath = configManager.resolveTemplate(buffer.getPath());
            String configJson = buildStreamConfig(resolvedPath, buffer);

            // Register the sink BEFORE open (the core binds it per stream at open time).
            StreamService.registerSink(batch -> d.drain(batch));
            this.streamService = StreamService.open(configJson);
            this.streamHandle = streamService.stream(DURABLE_STREAM);
            LOGGER.info("CloudWatch durable buffer open at {} (maxDiskBytes={}, onFull={}, fsync={})",
                    resolvedPath, buffer.getMaxDiskBytes(), buffer.getOnFull(), buffer.getFsync());
            return true;
        }
        catch (Throwable t)
        {
            LOGGER.warn("Durable CloudWatch buffer unavailable ({}); falling back to in-memory batching",
                    t.getMessage());
            closeDurable();
            return false;
        }
    }

    /** Build the single-stream {@code streaming} config JSON for the durable CloudWatch buffer. */
    private static String buildStreamConfig(String resolvedPath, BufferConfiguration buffer)
    {
        JsonObject sink = new JsonObject();
        sink.addProperty("type", "callback");
        sink.addProperty("id", DURABLE_STREAM);

        JsonObject buf = new JsonObject();
        buf.addProperty("type", "disk");
        buf.addProperty("path", resolvedPath);
        buf.addProperty("maxDiskBytes", buffer.getMaxDiskBytes());
        buf.addProperty("onFull", buffer.getOnFull());
        buf.addProperty("fsync", buffer.getFsync());

        JsonObject stream = new JsonObject();
        stream.addProperty("name", DURABLE_STREAM);
        stream.add("sink", sink);
        stream.add("buffer", buf);

        JsonArray streams = new JsonArray();
        streams.add(stream);
        JsonObject root = new JsonObject();
        root.add("streams", streams);
        return root.toString();
    }

    private void scheduleEmit()
    {
        // Reschedule on the same executor: cancel the current task and submit a new
        // one at the configured interval (executor reused; only shut down in close()).
        if (emitTask != null)
        {
            emitTask.cancel(false);
        }
        long periodMs = configManager.getMetricConfig().getIntervalSecs() * 1000L;
        // First flush after one interval (not at delay 0): nothing is buffered at
        // startup, so an immediate flush is pointless and only races with callers.
        emitTask = scheduler.scheduleAtFixedRate(this::runEmit, periodMs, periodMs, TimeUnit.MILLISECONDS);
    }

    @Override
    public void emitMetric(Metric metric, Map<String, Float> measureValues)
    {
        if (durable)
        {
            appendDurable(new PendingMetric(metric, measureValues));
            return;
        }
        // need to maintain a hashmap by namespace as we can only submit
        // from a single namespace at a time
        pendingMetrics.computeIfAbsent(metric.getNamespace(), k -> new ConcurrentLinkedQueue<>())
                     .add(new PendingMetric(metric, measureValues));
        LOGGER.info("Added {} metric to pending queue", metric.getName());
    }


    @Override
    public void emitMetricNow(Metric metric, Map<String, Float> measureValues)
    {
        if (durable)
        {
            // Durable mode: still goes through the buffer (always-buffer) — the engine drains it.
            appendDurable(new PendingMetric(metric, measureValues));
            return;
        }
        try
        {
            var data = new ArrayList<MetricDatum>();
            appendToPutMetricDataRequest(new PendingMetric(metric, measureValues), data);
            PutMetricDataRequest request = PutMetricDataRequest.builder()
                                                               .namespace(metric.getNamespace())
                                                               .metricData(data)
                                                               .build();
            cwClient.putMetricData(request);
        }
        catch (CloudWatchException e)
        {
            LOGGER.error("Error sending metric to CloudWatch. {}. Ignoring.", e.getMessage());
        }
        LOGGER.trace("Successfully sent {} metric to CloudWatch", metric.getName());
    }

    /** Serialize a pending metric's datums to {@code {namespace, datum}} records and append them. */
    private void appendDurable(PendingMetric pendingMetric)
    {
        var data = new ArrayList<MetricDatum>();
        appendToPutMetricDataRequest(pendingMetric, data);
        String namespace = pendingMetric.getMetric().getNamespace();
        for (MetricDatum datum : data)
        {
            try
            {
                byte[] payload = CloudWatchRecord.serialize(namespace, datum);
                // Partition key = namespace (so the drain can group by it).
                streamHandle.append(namespace, pendingMetric.getTimestamp().toEpochMilli(), payload);
            }
            catch (RuntimeException e)
            {
                LOGGER.error("Failed to append CloudWatch metric to durable buffer: {}", e.getMessage());
            }
        }
    }

    @Override
    public boolean onConfigurationChanged()
    {
        LOGGER.info("Configuration changed. Reconfiguring CloudWatch batch interval");
        if (!durable)
        {
            scheduleEmit();
        }
        return true;
    }

    @Override
    public void flush()
    {
        if (durable)
        {
            // Force the buffer durably to disk (the background engine handles the actual drain).
            try
            {
                if (streamHandle != null)
                {
                    streamHandle.flush();
                }
            }
            catch (RuntimeException e)
            {
                LOGGER.warn("Error flushing durable CloudWatch buffer: {}", e.getMessage());
            }
            return;
        }
        flushMetrics();
    }

    @Override
    public void close()
    {
        if (durable)
        {
            // Flush to disk + stop the engine; do NOT drain to cloud (backlog persists for restart).
            flush();
            closeDurable();
            StreamService.unregisterSink();
        }
        else
        {
            if (emitTask != null)
            {
                emitTask.cancel(false);
            }
            scheduler.shutdownNow();
        }
        try
        {
            cwClient.close();
        }
        catch (Exception e)
        {
            LOGGER.warn("Error closing CloudWatch client: {}", e.getMessage());
        }
    }

    private void closeDurable()
    {
        try
        {
            if (streamHandle != null)
            {
                streamHandle.close();
                streamHandle = null;
            }
        }
        catch (RuntimeException e)
        {
            LOGGER.warn("Error closing durable CloudWatch stream handle: {}", e.getMessage());
        }
        try
        {
            if (streamService != null)
            {
                streamService.close();
                streamService = null;
            }
        }
        catch (RuntimeException e)
        {
            LOGGER.warn("Error closing durable CloudWatch stream service: {}", e.getMessage());
        }
    }

    /** Datums dropped on drain for being outside CloudWatch's accept window (durable mode only). */
    public long getDroppedStale()
    {
        return drain == null ? 0L : drain.droppedStale();
    }

    /** The {@link CloudWatchSender} the durable drain calls — wraps the injected client. */
    private void putChunk(String namespace, List<MetricDatum> chunk)
    {
        sendBatch(namespace, chunk);
    }

    // CloudWatch PutMetricData accepts at most 1000 metric data items per request.
    private static final int MAX_DATUMS_PER_REQUEST = 1000;

    private void flushMetrics()
    {
        for (Map.Entry<String, ConcurrentLinkedQueue<PendingMetric>> entry : pendingMetrics.entrySet())
        {
            String namespace = entry.getKey();
            ConcurrentLinkedQueue<PendingMetric> pendingMetricQueue = entry.getValue();
            try
            {
                var data = new ArrayList<MetricDatum>();
                PendingMetric pendingMetric;
                while ((pendingMetric = pendingMetricQueue.poll()) != null)
                {
                    appendToPutMetricDataRequest(pendingMetric, data);
                }
                // CloudWatch PutMetricData accepts at most 1000 metric data items per call;
                // chunk the full datum list regardless of how it maps to individual metrics.
                for (int i = 0; i < data.size(); i += MAX_DATUMS_PER_REQUEST)
                {
                    sendBatch(namespace, data.subList(i, Math.min(i + MAX_DATUMS_PER_REQUEST, data.size())));
                }
            }
            catch (Exception e)
            {
                // Isolate failures per namespace so one failing namespace does not drop the others.
                LOGGER.error("Error sending pending metrics for namespace {} to CloudWatch. {}",
                        namespace, e.getMessage());
            }
        }
    }

    private void sendBatch(String namespace, Collection<MetricDatum> data)
    {
        PutMetricDataRequest request = PutMetricDataRequest.builder()
                                                           .namespace(namespace)
                                                           .metricData(data)
                                                           .build();
        cwClient.putMetricData(request);
        LOGGER.info("Successfully sent {} metric datums to CloudWatch namespace {}", data.size(), namespace);
    }

    private void appendToPutMetricDataRequest(PendingMetric pendingMetric, Collection<MetricDatum> data)
    {
        Map<String, Float> measureValues = pendingMetric.getMeasureValues();
        Collection<Dimension> dimensions = pendingMetric.getMetric().dimensionsAsCollection();
        Metric metric = pendingMetric.getMetric();
        Instant timestamp = pendingMetric.getTimestamp();

        addToDatum(data, measureValues, dimensions, metric, timestamp);

        if (metricConfig.getLargeFleetWorkaround())
        {
            dimensions = pendingMetric.getMetric().dimensionsAsCollection(true);
            addToDatum(data, measureValues, dimensions, metric, timestamp);
        }
    }

    private void addToDatum(Collection<MetricDatum> data, Map<String, Float> measureValues, Collection<Dimension> dimensions, Metric metric, Instant timestamp)
    {
        for (Map.Entry<String, Float> entry : measureValues.entrySet())
        {
            MetricDatum datum = MetricDatum.builder()
                                           .metricName(entry.getKey())
                                           .unit(metric.getMeasure(entry.getKey()).unit())
                                           .storageResolution(metric.getMeasure(entry.getKey()).storageResolution())
                                           .value(Double.valueOf(entry.getValue()))
                                           .timestamp(timestamp)
                                           .dimensions(dimensions)
                                           .build();
            data.add(datum);
        }
    }

    private static class PendingMetric
    {
        private final Metric metric;
        private final Map<String, Float> measureValues;

        private final Instant timestamp;

        public PendingMetric(Metric metric, Map<String, Float> measureValues)
        {
            this.metric = metric;
            this.measureValues = measureValues;
            this.timestamp = Instant.now();
        }

        public Metric getMetric()
        {
            return metric;
        }

        public Map<String, Float> getMeasureValues()
        {
            return measureValues;
        }

        public Instant getTimestamp()
        {
            return timestamp;
        }
    }

    /**
     * The periodic flush task. Guarded so an exception cannot propagate to the
     * scheduler and silently cancel future flushes.
     */
    private void runEmit()
    {
        try
        {
            flushMetrics();
        }
        catch (Exception e)
        {
            LOGGER.error("Unexpected error while emitting metrics to CloudWatch: {}", e.getMessage(), e);
        }
    }
}
