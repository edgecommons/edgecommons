/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.metrics.targets;

import com.aws.proserve.ggcommons.config.ConfigManager;
import com.aws.proserve.ggcommons.metrics.Metric;
import software.amazon.awssdk.services.cloudwatch.CloudWatchClient;
import software.amazon.awssdk.services.cloudwatch.model.CloudWatchException;
import software.amazon.awssdk.services.cloudwatch.model.Dimension;
import software.amazon.awssdk.services.cloudwatch.model.MetricDatum;
import software.amazon.awssdk.services.cloudwatch.model.PutMetricDataRequest;

import java.io.IOException;
import java.time.Instant;
import java.time.ZoneOffset;
import java.time.ZonedDateTime;
import java.time.format.DateTimeFormatter;
import java.util.*;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ConcurrentLinkedQueue;

public class CloudWatch extends MetricTarget
{

    private final CloudWatchClient cwClient;

    private final ConcurrentHashMap<String, ConcurrentLinkedQueue<PendingMetric>> pendingMetrics = new ConcurrentHashMap<>();

    private Timer metricEmitTimer;

    public CloudWatch(ConfigManager configManager)
    {
        super(configManager);
        cwClient = CloudWatchClient.builder().build();
        initEmitTimer();
    }

    private void initEmitTimer()
    {
        metricEmitTimer = new Timer("Metric Emit Timer", true);
        metricEmitTimer.scheduleAtFixedRate(new CloudWatch.PendingMetricEmitter(), 0,
                configManager.getMetricConfig().getIntervalSecs() * 1000L);
    }

    @Override
    public void emitMetric(Metric metric, Map<String, Float> measureValues)
    {
        // need to maintain a hashmap by namespace as we can only submit
        // from a single namespace at a time
        pendingMetrics.computeIfAbsent(metric.getNamespace(), k -> new ConcurrentLinkedQueue<>())
                     .add(new PendingMetric(metric, measureValues));
        LOGGER.info("Added {} metric to pending queue", metric.getName());
    }


    @Override
    public void emitMetricNow(Metric metric, Map<String, Float> measureValues)
    {
        try
        {
            Collection<MetricDatum> data = new ArrayList<>();
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

    @Override
    public boolean onConfigurationChanged()
    {
        LOGGER.info("Configuration changed. Reconfiguring CloudWatch batch interval");
        if (metricEmitTimer != null)
        {
            metricEmitTimer.cancel();
            metricEmitTimer.purge();
        }
        initEmitTimer();

        return true;
    }

    @Override
    public void close()
    {
        if (metricEmitTimer != null)
        {
            metricEmitTimer.cancel();
            metricEmitTimer.purge();
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
                Collection<MetricDatum> data = new ArrayList<>();
                PendingMetric pendingMetric;
                while ((pendingMetric = pendingMetricQueue.poll()) != null)
                {
                    appendToPutMetricDataRequest(pendingMetric, data);
                    if (data.size() >= MAX_DATUMS_PER_REQUEST)
                    {
                        sendBatch(namespace, data);
                        data = new ArrayList<>();
                    }
                }
                if (!data.isEmpty())
                {
                    sendBatch(namespace, data);
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
                                           .unit(metric.getMeasure(entry.getKey()).getUnit())
                                           .storageResolution(metric.getMeasure(entry.getKey()).getStorageResolution())
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
            this.timestamp = Instant.parse(ZonedDateTime.now(ZoneOffset.UTC).format(DateTimeFormatter.ISO_INSTANT));
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

    private class PendingMetricEmitter extends TimerTask
    {
        @Override
        public void run()
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
}
